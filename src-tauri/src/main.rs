use mach2::{
    kern_return::KERN_SUCCESS,
    mach_port::mach_port_deallocate,
    port::mach_port_t,
    traps::{mach_task_self, task_for_pid},
};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::ProtocolObject, sel, AllocAnyThread,
    DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSControlStateValueOff,
    NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSRunningApplication,
    NSStatusBar, NSStatusBarButton, NSTextField, NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use tauri::{ActivationPolicy, AppHandle, Manager};

mod mem;

struct Items {
    unlock: Retained<NSMenuItem>,
    resign: Retained<NSMenuItem>,
    cap: Retained<NSMenuItem>,
    button: Retained<NSStatusBarButton>,
    limit: Arc<AtomicU32>,
    path: Option<PathBuf>,
    run: Arc<Mutex<Option<Arc<mem::Run>>>>,
}

impl Items {
    fn sync(&self) {
        let pid = ready();
        let running = roblox().is_some();
        let signed = is_resigned();
        let mut run = self.run.lock().unwrap();
        if run.as_ref().is_some_and(|run| !run.is_on()) {
            run.take();
        }
        let on = run.is_some();
        drop(run);
        self.unlock.setEnabled(on || pid.is_some());
        self.unlock.setState(if on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.resign
            .setEnabled(!on && (!signed || running && pid.is_none()));
        self.cap.setTitle(&NSString::from_str(&label(
            self.limit.load(Ordering::Relaxed),
        )));
        self.button.setImage(Some(&icon(!on)));
    }

    fn toggle(&self) {
        let mut slot = self.run.lock().unwrap();
        if let Some(run) = slot.take() {
            run.stop();
        } else if let Some(pid) = ready() {
            match mem::Run::start(pid, self.limit.clone()) {
                Ok(run) => *slot = Some(run),
                Err(error) => fail("Unlock failed", &error),
            }
        }
        drop(slot);
        self.sync();
    }

    fn resign(&self) {
        match resign() {
            Ok(()) => self.sync(),
            Err(error) => fail("Resign failed", &error),
        }
    }

    fn cap(&self) {
        if let Some(fps) = ask(self.limit.load(Ordering::Relaxed)) {
            self.limit.store(fps, Ordering::Relaxed);
            save(self.path.as_deref(), fps);
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "Host"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Items]
    struct Host;

    impl Host {
        #[unsafe(method(toggle:))]
        fn toggle(&self, _sender: &NSMenuItem) {
            self.ivars().toggle();
        }

        #[unsafe(method(resign:))]
        fn resign(&self, _sender: &NSMenuItem) {
            self.ivars().resign();
        }

        #[unsafe(method(cap:))]
        fn cap(&self, _sender: &NSMenuItem) {
            self.ivars().cap();
        }
    }

    unsafe impl NSObjectProtocol for Host {}

    unsafe impl NSMenuDelegate for Host {
        #[unsafe(method(menuWillOpen:))]
        #[allow(non_snake_case)]
        fn menuWillOpen(&self, _menu: &NSMenu) {
            self.ivars().sync();
        }
    }
);

impl Host {
    fn new(mtm: MainThreadMarker, items: Items) -> Retained<Self> {
        let this = mtm.alloc().set_ivars(items);
        unsafe { msg_send![super(this), init] }
    }
}

fn fail(title: &str, error: &str) {
    let mtm = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(error));
    unsafe { alert.setIcon(logo().as_deref()) };
    alert.runModal();
}

// yes this is how i do it. i know theres a plugin and i DONT care.
fn settings(handle: &AppHandle) -> Option<PathBuf> {
    let dir = handle.path().app_data_dir().ok()?;
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("fps"))
}

fn load(path: Option<&Path>) -> u32 {
    path.and_then(|path| fs::read_to_string(path).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(mem::MAX)
        .max(1)
}

fn save(path: Option<&Path>, limit: u32) {
    if let Some(path) = path {
        let _ = fs::write(path, limit.to_string());
    }
}

fn label(fps: u32) -> String {
    format!("FPS Limit: {fps}")
}

fn logo() -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(include_bytes!("../icons/icon.png"));
    NSImage::initWithData(NSImage::alloc(), &data)
}

fn ask(current: u32) -> Option<u32> {
    let mtm = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("FPS Limit"));
    unsafe { alert.setIcon(logo().as_deref()) };
    alert.addButtonWithTitle(&NSString::from_str("Set"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    let field = NSTextField::textFieldWithString(&NSString::from_str(&current.to_string()), mtm);
    field.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(220.0, 24.0),
    ));
    alert.setAccessoryView(Some(&field));
    alert.window().setInitialFirstResponder(Some(&field));
    if alert.runModal() != NSAlertFirstButtonReturn {
        return None;
    }
    // type 5000 if you want, it will proudly say 5000
    field
        .stringValue()
        .to_string()
        .trim()
        .parse::<u32>()
        .ok()
        .map(|fps| fps.max(1))
}

fn icon(locked: bool) -> Retained<NSImage> {
    let name = if locked {
        "lock.display"
    } else {
        "lock.open.display"
    };
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )
    .unwrap();
    image.setTemplate(true);
    image
}

fn add(menu: &NSMenu, title: &str) -> Retained<NSMenuItem> {
    unsafe {
        menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str(title),
            None,
            &NSString::new(),
        )
    }
}

fn tray(handle: &AppHandle) {
    let path = settings(handle);
    let mtm = MainThreadMarker::new().unwrap();
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(NSVariableStatusItemLength);
    let button = item.button(mtm).unwrap();
    let image = icon(true);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let limit = Arc::new(AtomicU32::new(load(path.as_deref())));
    let cap = add(&menu, &label(limit.load(Ordering::Relaxed)));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let unlock = add(&menu, "Unlock FPS");
    unlock.setEnabled(false);
    unlock.setState(NSControlStateValueOff);
    let resign = add(&menu, "Resign Roblox");
    resign.setEnabled(true);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let app = NSApplication::sharedApplication(mtm);
    let quit = unsafe {
        menu.addItemWithTitle_action_keyEquivalent(
            &NSString::from_str("Quit"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        )
    };

    unsafe { quit.setTarget(Some(&app)) };
    button.setImage(Some(&image));
    item.setMenu(Some(&menu));
    let items = Items {
        unlock,
        resign,
        cap,
        button,
        limit,
        path,
        run: Arc::new(Mutex::new(None)),
    };
    let host = Host::new(mtm, items);
    let ivars = host.ivars();
    unsafe {
        ivars.unlock.setTarget(Some(&host));
        ivars.unlock.setAction(Some(sel!(toggle:)));
        ivars.resign.setTarget(Some(&host));
        ivars.resign.setAction(Some(sel!(resign:)));
        ivars.cap.setTarget(Some(&host));
        ivars.cap.setAction(Some(sel!(cap:)));
    }
    menu.setDelegate(Some(ProtocolObject::from_ref(&*host)));
    // tray will be killed if this drops
    std::mem::forget(item);
    std::mem::forget(host);
}

fn ready() -> Option<i32> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
        "com.roblox.RobloxPlayer",
    ))
    .iter()
    .find_map(|app| {
        let pid = app.processIdentifier();
        if pid < 1 {
            None
        } else {
            let mut task: mach_port_t = 0;
            let result = unsafe { task_for_pid(mach_task_self(), pid, &mut task) };
            if result == KERN_SUCCESS {
                unsafe { mach_port_deallocate(mach_task_self(), task) };
                Some(pid)
            } else {
                None
            }
        }
    })
}

fn roblox() -> Option<i32> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
        "com.roblox.RobloxPlayer",
    ))
    .iter()
    .map(|app| app.processIdentifier())
    .find(|pid| *pid > 0)
}

fn is_resigned() -> bool {
    let Ok(output) = Command::new("/usr/bin/codesign")
        .args([
            "-d",
            "--entitlements",
            ":-",
            "/Applications/Roblox.app/Contents/MacOS/RobloxPlayer",
        ])
        .output()
    else {
        return false;
    };
    let mut data = output.stdout;
    data.extend_from_slice(&output.stderr);
    String::from_utf8_lossy(&data).contains("com.apple.security.get-task-allow")
}

fn resign() -> Result<(), String> {
    let _ = Command::new("/usr/bin/pkill")
        .args(["-x", "RobloxPlayer"])
        .status();
    for _ in 0..20 {
        if roblox().is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if roblox().is_some() {
        return Err("Roblox did not quit".to_owned());
    }
    let path = std::env::temp_dir().join(format!("fps-unlocker-{}.plist", std::process::id()));
    fs::write(
        &path,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>com.apple.security.get-task-allow</key><true/></dict></plist>"#,
    )
    .map_err(|e| e.to_string())?;
    let out = Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", "-", "--entitlements"])
        .arg(&path)
        .arg("/Applications/Roblox.app/Contents/MacOS/RobloxPlayer")
        .output()
        .map_err(|e| e.to_string())?;
    let _ = fs::remove_file(path);
    if !out.status.success() || !is_resigned() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            app.set_activation_policy(ActivationPolicy::Accessory);
            tray(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start");
}
