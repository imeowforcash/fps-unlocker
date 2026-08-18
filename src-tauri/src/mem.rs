use mach2::{
    kern_return::KERN_SUCCESS,
    mach_port::mach_port_deallocate,
    message::mach_msg_type_number_t,
    port::mach_port_t,
    task::task_info,
    task_info::{task_dyld_info, TASK_DYLD_INFO},
    traps::{mach_task_self, task_for_pid},
    vm::{mach_vm_read_overwrite, mach_vm_write},
    vm_types::{mach_vm_address_t, mach_vm_size_t, vm_offset_t},
};
use objc2::ClassType;
use objc2_quartz_core::CAMetalLayer;
use std::{
    collections::HashSet,
    mem::{size_of, MaybeUninit},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

extern "C" {
    fn vm_region_recurse_64(
        target: mach_port_t,
        address: *mut mach_vm_address_t,
        size: *mut mach_vm_size_t,
        depth: *mut u32,
        info: *mut i32,
        info_count: *mut u32,
    ) -> i32;
    static objc_debug_isa_class_mask: usize;
}

const MH_MAGIC_64: u32 = 0xfeedfacf;
const LC_SEGMENT_64: u32 = 0x19;

struct Task(mach_port_t);

impl Drop for Task {
    fn drop(&mut self) {
        unsafe { mach_port_deallocate(mach_task_self(), self.0) };
    }
}

struct Process {
    task: Task,
    base: u64,
}

#[derive(Clone, Copy)]
struct Region {
    start: u64,
    size: u64,
    prot: i32,
}

#[derive(Clone, Copy)]
struct Metal {
    class: u64,
    mask: u64,
    field: u64,
    inner: u64,
    bit: u8,
    base: u8,
    off: u8,
}

#[derive(Clone, Copy)]
struct Targets {
    interval: u64,
    sync: u64,
    bit: u8,
    off: u8,
}

struct Saved {
    interval: f64,
    sync: u8,
}

pub struct Run {
    process: Process,
    targets: Targets,
    saved: Saved,
    on: AtomicBool,
    gate: Mutex<()>,
}

impl Process {
    fn attach(pid: i32) -> Result<Self, String> {
        let mut task = 0;
        let kr = unsafe { task_for_pid(mach_task_self(), pid, &mut task) };
        if kr != KERN_SUCCESS {
            return Err(format!("task_for_pid failed: {kr}"));
        }
        let mut process = Self {
            task: Task(task),
            base: 0,
        };
        process.base = process.dyld_base().ok_or("Roblox image not found")?;
        Ok(process)
    }

    fn read<T: Copy>(&self, addr: u64) -> Result<T, String> {
        let mut val = MaybeUninit::<T>::uninit();
        let mut out = 0;
        let kr = unsafe {
            mach_vm_read_overwrite(
                self.task.0,
                addr,
                size_of::<T>() as u64,
                val.as_mut_ptr() as u64,
                &mut out,
            )
        };
        if kr != KERN_SUCCESS || out as usize != size_of::<T>() {
            return Err(format!("read failed: {kr}"));
        }
        Ok(unsafe { val.assume_init() })
    }

    fn read_bytes(&self, addr: u64, data: &mut [u8]) -> Result<(), String> {
        read_task(self.task.0, addr, data)
    }

    fn write<T: Copy>(&self, addr: u64, val: T) -> Result<(), String> {
        write_task(self.task.0, addr, val)
    }

    fn regions(&self) -> Vec<Region> {
        let mut regions = Vec::new();
        let mut addr = 1;
        loop {
            let mut size = 0;
            let mut depth = 0;
            let mut info = [0u32; 32];
            let mut count = info.len() as u32;
            let kr = unsafe {
                vm_region_recurse_64(
                    self.task.0,
                    &mut addr,
                    &mut size,
                    &mut depth,
                    info.as_mut_ptr() as *mut i32,
                    &mut count,
                )
            };
            if kr != KERN_SUCCESS {
                break;
            }
            if size > 0 {
                regions.push(Region {
                    start: addr,
                    size,
                    prot: info[0] as i32,
                });
            }
            let next = addr.saturating_add(size);
            if next <= addr {
                break;
            }
            addr = next;
        }
        regions
    }

    fn image_regions(&self) -> Result<Vec<Region>, String> {
        let header = self.read::<[u32; 8]>(self.base)?;
        if header[0] != MH_MAGIC_64 || header[4] > 1024 || header[5] > 0x100000 {
            return Err("bad macho".to_owned());
        }
        let mut data = vec![0u8; header[5] as usize];
        self.read_bytes(self.base + 32, &mut data)?;
        let mut at = 0usize;
        let mut segments = Vec::new();
        let mut text = None;
        for _ in 0..header[4] {
            if at + 8 > data.len() {
                return Err("bad macho".to_owned());
            }
            let cmd = u32::from_le_bytes(data[at..at + 4].try_into().unwrap());
            let len = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
            if len < 8 || at + len > data.len() {
                return Err("bad macho".to_owned());
            }
            if cmd == LC_SEGMENT_64 && len >= 72 {
                let name = &data[at + 8..at + 24];
                let vm = u64::from_le_bytes(data[at + 24..at + 32].try_into().unwrap());
                let size = u64::from_le_bytes(data[at + 32..at + 40].try_into().unwrap());
                let prot = i32::from_le_bytes(data[at + 60..at + 64].try_into().unwrap());
                if name.starts_with("__TEXT".as_bytes()) && name[6] == 0 {
                    text = Some(vm);
                }
                segments.push((vm, size, prot));
            }
            at += len;
        }
        let slide = self.base.wrapping_sub(text.ok_or("bad macho")?);
        Ok(segments
            .into_iter()
            .map(|(start, size, prot)| Region {
                start: start.wrapping_add(slide),
                size,
                prot,
            })
            .collect())
    }

    fn read_cstr(&self, addr: u64) -> Option<String> {
        let mut bytes = Vec::new();
        for step in 0..8 {
            let mut part = [0u8; 32];
            self.read_bytes(addr + step * 32, &mut part).ok()?;
            if let Some(end) = part.iter().position(|b| *b == 0) {
                bytes.extend_from_slice(&part[..end]);
                return String::from_utf8(bytes).ok();
            }
            bytes.extend_from_slice(&part);
        }
        None
    }

    fn dyld_base(&self) -> Option<u64> {
        let mut info: task_dyld_info = unsafe { std::mem::zeroed() };
        let mut count = (size_of::<task_dyld_info>() / size_of::<u32>()) as mach_msg_type_number_t;
        let kr = unsafe {
            task_info(
                self.task.0,
                TASK_DYLD_INFO,
                &mut info as *mut _ as *mut i32,
                &mut count,
            )
        };
        if kr != KERN_SUCCESS {
            return None;
        }
        let n = self.read::<u32>(info.all_image_info_addr + 4).ok()?;
        let list = self.read::<u64>(info.all_image_info_addr + 8).ok()?;
        for i in 0..n as u64 {
            let item = list + i * 24;
            let load = self.read::<u64>(item).ok()?;
            let path = self.read::<u64>(item + 8).ok()?;
            if self
                .read_cstr(path)
                .is_some_and(|path| path.ends_with("MacOS/RobloxPlayer"))
            {
                return Some(load);
            }
        }
        None
    }

    fn text(&self, addr: u64) -> Option<String> {
        let bytes = self.read::<[u8; 24]>(addr).ok()?;
        let arm = if bytes[23] & 0x80 == 0 {
            let len = (bytes[23] & 0x7f) as usize;
            (len <= 23).then(|| bytes[..len].to_vec())
        } else {
            let ptr = u64::from_le_bytes(bytes[..8].try_into().unwrap());
            let len = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
            self.long_text(ptr, len)
        };
        let intel = if bytes[0] & 1 == 0 {
            let len = (bytes[0] >> 1) as usize;
            (len <= 23).then(|| bytes[1..1 + len].to_vec())
        } else {
            let len = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
            let ptr = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
            self.long_text(ptr, len)
        };
        [arm, intel]
            .into_iter()
            .flatten()
            .find_map(|bytes| {
                String::from_utf8(bytes).ok().filter(|s| {
                    (3..48).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
                })
            })
    }

    fn long_text(&self, ptr: u64, len: usize) -> Option<Vec<u8>> {
        if !is_ptr(ptr) || len > 128 {
            return None;
        }
        let mut bytes = vec![0u8; len];
        self.read_bytes(ptr, &mut bytes).ok()?;
        Some(bytes)
    }

    fn jobs(&self, head: &[u8]) -> Option<u64> {
        for off in (0..head.len().saturating_sub(16)).step_by(8) {
            let begin = u64::from_le_bytes(head[off..off + 8].try_into().unwrap());
            let end = u64::from_le_bytes(head[off + 8..off + 16].try_into().unwrap());
            if !is_ptr(begin) || end <= begin || end - begin > 0x10000 || (end - begin) % 16 != 0 {
                continue;
            }
            let n = ((end - begin) / 16) as usize;
            if !(20..300).contains(&n) {
                continue;
            }
            let mut list = vec![0u8; (end - begin) as usize];
            if self.read_bytes(begin, &mut list).is_err() {
                continue;
            }
            let jobs: Vec<u64> = list
                .chunks_exact(16)
                .map(|item| u64::from_le_bytes(item[..8].try_into().unwrap()))
                .filter(|ptr| is_ptr(*ptr))
                .collect();
            if jobs.len() * 10 < n * 9 {
                continue;
            }
            for name_off in (0..0x80).step_by(8) {
                let mut good = 0;
                let mut jobish = 0;
                for job in &jobs {
                    if let Some(name) = self.text(*job + name_off) {
                        good += 1;
                        // if the names are like "Job" "Render" "Physics" its PROBABLY the scheduler
                        if ["Job", "Task", "Render", "Physics", "Heartbeat", "Replicat"]
                            .iter()
                            .any(|word| name.contains(word))
                        {
                            jobish += 1;
                        }
                    }
                }
                if good * 10 >= n * 7 && jobish >= 5 {
                    return Some(off as u64);
                }
            }
        }
        None
    }

    fn scheduler(&self, image: &[Region], heap: &[Region]) -> Result<(u64, u64), String> {
        let mut seen = HashSet::new();
        let mut chunk = vec![0u8; 0x100000];
        for region in image.iter().filter(|r| r.prot & 1 != 0 && r.prot & 4 == 0) {
            let mut done = 0;
            while done < region.size {
                let len = (region.size - done).min(chunk.len() as u64) as usize;
                if self
                    .read_bytes(region.start + done, &mut chunk[..len])
                    .is_ok()
                {
                    for item in chunk[..len].chunks_exact(8) {
                        let ptr = u64::from_le_bytes(item.try_into().unwrap());
                        if !seen.insert(ptr) || !inside(heap, ptr, 0x200) {
                            continue;
                        }
                        let mut head = [0u8; 0x200];
                        if self.read_bytes(ptr, &mut head).is_err() {
                            continue;
                        }
                        let Some(jobs) = self.jobs(&head) else {
                            continue;
                        };
                        let mut intervals = Vec::new();
                        for off in (0..jobs as usize).step_by(8) {
                            let val = f64::from_le_bytes(head[off..off + 8].try_into().unwrap());
                            if !val.is_finite() || val <= 0.0 {
                                continue;
                            }
                            let fps = 1.0 / val;
                            if (20.0..=1000.0).contains(&fps) && (fps - fps.round()).abs() < 0.05 {
                                intervals.push(off as u64);
                            }
                        }
                        if let Some(off) = intervals.into_iter().max() {
                            return Ok((ptr, off));
                        }
                    }
                }
                done += len as u64;
            }
        }
        Err("scheduler not found".to_owned())
    }

    fn sync(&self, metal: Metal, heap: &[Region]) -> Result<u64, String> {
        let mut chunk = vec![0u8; 0x100000];
        for region in heap
            .iter()
            .filter(|r| r.prot & 3 == 3 && r.size < 200_000_000)
        {
            let mut done = 0;
            while done < region.size {
                let len = (region.size - done).min(chunk.len() as u64) as usize;
                if self
                    .read_bytes(region.start + done, &mut chunk[..len])
                    .is_ok()
                {
                    for (i, item) in chunk[..len].chunks_exact(8).enumerate() {
                        let isa = u64::from_le_bytes(item.try_into().unwrap());
                        if isa & metal.mask != metal.class & metal.mask {
                            continue;
                        }
                        let layer = region.start + done + (i * 8) as u64;
                        let Ok(data) = self.read::<u64>(layer + metal.field) else {
                            continue;
                        };
                        if !inside(heap, data, metal.inner + 1) {
                            continue;
                        }
                        let sync = data + metal.inner;
                        let Ok(value) = self.read::<u8>(sync) else {
                            continue;
                        };
                        if value & !metal.bit == metal.base & !metal.bit {
                            return Ok(sync);
                        }
                    }
                }
                done += len as u64;
            }
        }
        Err("Metal layer not found".to_owned())
    }
}

impl Run {
    pub fn start(pid: i32) -> Result<Arc<Self>, String> {
        let metal = metal()?;
        let process = Process::attach(pid)?;
        let heap = process.regions();
        let image = process.image_regions()?;
        let (sched, interval) = process.scheduler(&image, &heap)?;
        let sync = process.sync(metal, &heap)?;
        let targets = Targets {
            interval: sched + interval,
            sync,
            bit: metal.bit,
            off: metal.off,
        };
        let saved = Saved {
            interval: process.read(targets.interval)?,
            sync: process.read(targets.sync)?,
        };
        let run = Arc::new(Self {
            process,
            targets,
            saved,
            on: AtomicBool::new(true),
            gate: Mutex::new(()),
        });
        run.apply()?;
        let worker = run.clone();
        thread::spawn(move || {
            // roblox will revert if we dont keep writing it
            while worker.on.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                let _guard = worker.gate.lock().unwrap();
                if worker.on.load(Ordering::Relaxed) && worker.apply().is_err() {
                    worker.on.store(false, Ordering::Relaxed);
                    worker.restore();
                }
            }
        });
        Ok(run)
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        let _guard = self.gate.lock().unwrap();
        if self.on.swap(false, Ordering::Relaxed) {
            self.restore();
        }
    }

    fn apply(&self) -> Result<(), String> {
        self.process.write(self.targets.interval, 1.0f64 / 1000.0)?;
        let value = self.process.read::<u8>(self.targets.sync)?;
        let value = (value & !self.targets.bit) | (self.targets.off & self.targets.bit);
        self.process.write(self.targets.sync, value)
    }

    fn restore(&self) {
        let _ = self
            .process
            .write(self.targets.interval, self.saved.interval);
        if let Ok(value) = self.process.read::<u8>(self.targets.sync) {
            let value = (value & !self.targets.bit) | (self.saved.sync & self.targets.bit);
            let _ = self.process.write(self.targets.sync, value);
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.stop();
    }
}

fn metal() -> Result<Metal, String> {
    let layer = CAMetalLayer::new();
    layer.setDisplaySyncEnabled(true);
    let starts_on = layer.displaySyncEnabled();
    let object = &*layer as *const CAMetalLayer as u64;
    let class = CAMetalLayer::class() as *const _ as u64;
    let mask = unsafe { objc_debug_isa_class_mask as u64 };
    let size = CAMetalLayer::class().instance_size().min(0x100);
    let mut object_data = vec![0u8; size];
    read_task(unsafe { mach_task_self() }, object, &mut object_data)?;
    let mut before = Vec::new();
    for off in (8..size.saturating_sub(7)).step_by(8) {
        let ptr = u64::from_le_bytes(object_data[off..off + 8].try_into().unwrap());
        if !is_ptr(ptr) {
            continue;
        }
        let mut data = vec![0u8; 0x300];
        if read_task(unsafe { mach_task_self() }, ptr, &mut data).is_ok() {
            before.push((off as u64, ptr, data));
        }
    }
    layer.setDisplaySyncEnabled(false);
    let turns_off = !layer.displaySyncEnabled();
    let pointers = before.len();
    let mut changed = Vec::new();
    for (field, ptr, on) in before {
        let mut off = vec![0u8; on.len()];
        if read_task(unsafe { mach_task_self() }, ptr, &mut off).is_err() {
            continue;
        }
        layer.setDisplaySyncEnabled(true);
        let mut again = vec![0u8; on.len()];
        let ok = read_task(unsafe { mach_task_self() }, ptr, &mut again).is_ok();
        layer.setDisplaySyncEnabled(false);
        if !ok {
            continue;
        }
        for inner in 0..on.len() {
            let bit = on[inner] ^ off[inner];
            if on[inner] == again[inner] && bit.is_power_of_two() {
                changed.push((field, ptr, inner as u64, bit, on[inner], off[inner]));
            }
        }
    }
    let changes = changed.len();
    if mask != 0 && starts_on && turns_off && changes == 1 {
        let (field, _, inner, bit, on, off) = changed[0];
        return Ok(Metal {
            class,
            mask,
            field,
            inner,
            bit,
            base: on,
            off,
        });
    }
    Err(format!(
        "display sync field not found: size={size:#x} pointers={pointers} setter={starts_on}/{turns_off} changes={changes}"
    ))
}

fn read_task(task: mach_port_t, addr: u64, data: &mut [u8]) -> Result<(), String> {
    let mut out = 0;
    let kr = unsafe {
        mach_vm_read_overwrite(
            task,
            addr,
            data.len() as u64,
            data.as_mut_ptr() as u64,
            &mut out,
        )
    };
    if kr != KERN_SUCCESS || out as usize != data.len() {
        Err(format!("read failed: {kr}"))
    } else {
        Ok(())
    }
}

fn write_task<T: Copy>(task: mach_port_t, addr: u64, val: T) -> Result<(), String> {
    let kr = unsafe {
        mach_vm_write(
            task,
            addr,
            &val as *const T as vm_offset_t,
            size_of::<T>() as u32,
        )
    };
    if kr == KERN_SUCCESS {
        Ok(())
    } else {
        Err(format!("write failed: {kr}"))
    }
}

fn is_ptr(ptr: u64) -> bool {
    ptr >= 0x1_0000_0000 && ptr < 0x8000_0000_0000 && ptr & 7 == 0
}

fn inside(regions: &[Region], addr: u64, size: u64) -> bool {
    regions.iter().any(|region| {
        region.prot & 1 != 0
            && addr >= region.start
            && addr.saturating_add(size) <= region.start.saturating_add(region.size)
    })
}
