use jni::{
    JNIEnv, JavaVM,
    objects::{GlobalRef, JObject, JString},
};
use std::{
    os::fd::RawFd,
    sync::{LazyLock, OnceLock},
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

/// An event handed over from the Android `MainActivity`.
pub enum UsbEvent {
    /// `UsbManager.openDevice()` succeeded.
    /// Carries `(fd, vendor_id, product_id)` of an opened, permission-granted USB device.
    DeviceReady(RawFd, u16, u16),
    /// A human-readable log to be shown on screen.
    Log(String),
}

struct EventChannel {
    tx: mpsc::UnboundedSender<UsbEvent>,
    rx: AsyncMutex<mpsc::UnboundedReceiver<UsbEvent>>,
}

static EVENT_CHANNEL: LazyLock<EventChannel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::unbounded_channel();
    EventChannel {
        tx,
        rx: AsyncMutex::new(rx),
    }
});

static JVM: OnceLock<JavaVM> = OnceLock::new();
static MAIN_ACTIVITY_CLASS: OnceLock<GlobalRef> = OnceLock::new();

/// Guard that notifies the Kotlin side when the native USB session ends.
pub struct SessionGuard(());

impl SessionGuard {
    pub fn new() -> Self {
        Self(())
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        notify_session_ended();
    }
}

/// Clears a pending Java exception left behind by a failed JNI call.
fn clear_pending_exception(env: &mut JNIEnv<'_>) {
    if let Ok(true) = env.exception_check() {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn cache_main_activity_class(env: &mut JNIEnv<'_>) {
    if MAIN_ACTIVITY_CLASS.get().is_none() {
        match env.find_class("dev/dioxus/main/MainActivity") {
            Ok(cls) => match env.new_global_ref(&cls) {
                Ok(global) => {
                    let _ = MAIN_ACTIVITY_CLASS.set(global);
                }
                Err(e) => {
                    log::error!("cache_main_activity_class: failed to create global ref: {e}");
                    clear_pending_exception(env);
                }
            },
            Err(e) => {
                log::error!("cache_main_activity_class: find_class failed: {e}");
                clear_pending_exception(env);
            }
        }
    }
}

/// Notifies the Kotlin `MainActivity` that the native USB session has ended
/// and that it is now safe to close the `UsbDeviceConnection`.
fn notify_session_ended() {
    let jvm = match JVM.get() {
        Some(jvm) => jvm,
        None => {
            log::error!("notify_session_ended: JVM not initialized");
            return;
        }
    };
    let cls = match MAIN_ACTIVITY_CLASS.get() {
        Some(cls) => cls,
        None => {
            log::error!("notify_session_ended: MainActivity class not cached yet");
            return;
        }
    };
    let mut env = match jvm.attach_current_thread() {
        Ok(env) => env,
        Err(e) => {
            log::error!("notify_session_ended: failed to attach to JVM: {e}");
            return;
        }
    };
    if let Err(e) = env.call_static_method(cls, "onNativeUsbSessionEnded", "()V", &[]) {
        log::error!("notify_session_ended: call_static_method failed: {e}");
        clear_pending_exception(&mut env);
    }
}

pub async fn next_event() -> UsbEvent {
    EVENT_CHANNEL
        .rx
        .lock()
        .await
        .recv()
        .await
        .expect("USB event channel closed unexpectedly")
}

/// Called from Kotlin (`MainActivity.nativeUsbDeviceReady`) once the user has
/// granted USB permission for the P2Pro and `UsbManager.openDevice()` has
/// handed back a file descriptor for it.
///
/// Java signature: `private external fun nativeUsbDeviceReady(fd: Int, vendorId: Int, productId: Int)`
/// on `dev.dioxus.main.MainActivity`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeUsbDeviceReady<'local>(
    mut env: JNIEnv<'local>,
    _this: JObject<'local>,
    fd: i32,
    vendor_id: i32,
    product_id: i32,
) {
    cache_main_activity_class(&mut env);
    if let Ok(jvm) = env.get_java_vm() {
        let _ = JVM.set(jvm);
    }
    let _ = EVENT_CHANNEL.tx.send(UsbEvent::DeviceReady(
        fd as RawFd,
        vendor_id as u16,
        product_id as u16,
    ));
}

/// Called from Kotlin (`MainActivity.nativeUsbLog`) to mirror a USB status /
/// debug line to the native side, where it is rendered on screen.
///
/// Java signature: `private external fun nativeUsbLog(msg: String)`
/// on `dev.dioxus.main.MainActivity`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeUsbLog<'local>(
    mut env: JNIEnv<'local>,
    _this: JObject<'local>,
    msg: JString<'local>,
) {
    cache_main_activity_class(&mut env);
    if let Ok(jvm) = env.get_java_vm() {
        let _ = JVM.set(jvm);
    }
    if let Ok(java_str) = env.get_string(&msg) {
        let _ = EVENT_CHANNEL.tx.send(UsbEvent::Log(java_str.into()));
    }
}
