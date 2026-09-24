use crate::platform::events::Event;
pub fn install(notify: fn(Event)) -> anyhow::Result<()> {
    use block2::RcBlock;
    use core_foundation::{
        array::CFArray,
        runloop::{kCFRunLoopCommonModes, CFRunLoop},
        string::CFString,
    };
    use objc2_app_kit::{
        NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
    };
    use system_configuration::dynamic_store::{
        SCDynamicStoreBuilder, SCDynamicStoreCallBackContext,
    };

    // Install on Tauri's main thread; its AppKit run loop delivers both sources.
    let store = SCDynamicStoreBuilder::new("Porthop network recovery")
        .callback_context(SCDynamicStoreCallBackContext {
            callout: |_, _, notify: &mut fn(Event)| {
                log::debug!("Network configuration changed; notifying recovery tasks");
                notify(Event::NetworkChanged);
            },
            info: notify,
        })
        .build()
        .ok_or_else(|| anyhow::anyhow!("Cannot create network observer"))?;
    let patterns = CFArray::from_CFTypes(&[
        CFString::from("State:/Network/Global/.*"),
        CFString::from("State:/Network/Service/.*/(IPv4|IPv6|DNS)"),
        CFString::from("State:/Network/Interface/.*/Link"),
    ]);
    anyhow::ensure!(
        store.set_notification_keys(&CFArray::<CFString>::from_CFTypes(&[]), &patterns),
        "Cannot subscribe to network changes"
    );
    let source = store
        .create_run_loop_source()
        .ok_or_else(|| anyhow::anyhow!("Cannot create network event source"))?;
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // The notification center retains these observers for the application lifetime.
    // Blocks capture only a bool and access a thread-safe watch sender.
    unsafe {
        for (name, sleeping) in [
            (NSWorkspaceWillSleepNotification, true),
            (NSWorkspaceDidWakeNotification, false),
        ] {
            let callback = RcBlock::new(move |_| {
                log::info!(
                    "System {}; notifying recovery tasks",
                    if sleeping { "sleeping" } else { "awake" }
                );
                notify(if sleeping { Event::Sleep } else { Event::Wake });
            });
            let _observer = center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                None,
                &callback,
            );
        }
        CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes);
    }
    // The run loop retains the source, which retains its dynamic store context.
    Ok(())
}
