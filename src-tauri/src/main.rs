use mach2::{
    kern_return::KERN_SUCCESS,
    mach_port::mach_port_deallocate,
    port::mach_port_t,
    traps::{mach_task_self, task_for_pid},
};
use objc2::{
    define_class, msg_send, rc::Retained, runtime::ProtocolObject, sel, DefinedClass,
    MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu,
    NSMenuDelegate, NSMenuItem, NSRunningApplication, NSStatusBar, NSStatusBarButton,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tauri::ActivationPolicy;

mod mem;

struct Items {
    unlock: Retained<NSMenuItem>,
    resign: Retained<NSMenuItem>,
    button: Retained<NSStatusBarButton>,
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
        self.button.setImage(Some(&icon(!on)));
    }

    fn toggle(&self) {
        let mut slot = self.run.lock().unwrap();
        if let Some(run) = slot.take() {
            run.stop();
        } else if let Some(pid) = ready() {
            match mem::Run::start(pid) {
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
    alert.runModal();
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

fn tray() {
    let mtm = MainThreadMarker::new().unwrap();
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(NSVariableStatusItemLength);
    let button = item.button(mtm).unwrap();
    let image = icon(true);
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
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
        button,
        run: Arc::new(Mutex::new(None)),
    };
    let host = Host::new(mtm, items);
    let ivars = host.ivars();
    unsafe {
        ivars.unlock.setTarget(Some(&host));
        ivars.unlock.setAction(Some(sel!(toggle:)));
        ivars.resign.setTarget(Some(&host));
        ivars.resign.setAction(Some(sel!(resign:)));
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
            tray();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to start");
}
