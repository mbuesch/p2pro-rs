use jni::{
    Env, EnvUnowned, JavaVM,
    errors::ThrowRuntimeExAndDefault,
    objects::{JObject, JString, JValue},
    jni_str,
    jni_sig,
};
use std::{
    os::fd::RawFd,
    sync::{LazyLock, OnceLock},
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use anyhow as ah;

/// An event handed over from the Android `MainActivity`.
pub enum UsbEvent {
    /// `UsbManager.openDevice()` succeeded.
    /// Carries `(fd, vendor_id, product_id, session_token)` of an opened, permission-granted USB device.
    DeviceReady(RawFd, u16, u16, i64),
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

/// Guard that notifies the Kotlin side when the native USB session ends.
pub struct SessionGuard(i64);

impl SessionGuard {
    pub fn new(session_token: i64) -> Self {
        Self(session_token)
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        notify_session_ended(self.0);
    }
}

/// Clears a pending Java exception left behind by a failed JNI call.
fn clear_pending_exception(env: &Env<'_>) {
    if env.exception_check() {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

/// Notifies the Kotlin `MainActivity` that the native USB session has ended.
fn notify_session_ended(session_token: i64) {
    let jvm = match JVM.get() {
        Some(jvm) => jvm,
        None => {
            log::error!("notify_session_ended: JVM not initialized");
            return;
        }
    };
    let _ = jvm.with_local_frame(4096, |env| -> ah::Result<()> {
        if let Ok(cls) = env.find_class(jni_str!("dev/dioxus/main/MainActivity")) {
            let args = [JValue::Long(session_token)];
            if let Err(e) = env.call_static_method(cls, jni_str!("onNativeUsbSessionEnded"), jni_sig!("(J)V"), &args) {
                log::error!("notify_session_ended: call_static_method failed: {e}");
                clear_pending_exception(env);
            }
        }
        Ok(())
    });
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
/// Java signature: `private external fun nativeUsbDeviceReady(fd: Int, vendorId: Int, productId: Int, token: Long)`
/// on `dev.dioxus.main.MainActivity`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeUsbDeviceReady<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
    fd: i32,
    vendor_id: i32,
    product_id: i32,
    token: i64,
) {
    env.with_env(|env| -> Result<_, jni::errors::Error> {
        if let Ok(jvm) = env.get_java_vm() {
            let _ = JVM.set(jvm);
        }
        let _ = EVENT_CHANNEL.tx.send(UsbEvent::DeviceReady(
            fd as RawFd,
            vendor_id as u16,
            product_id as u16,
            token,
        ));
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// Called from Kotlin (`MainActivity.nativeUsbLog`) to mirror a USB status /
/// debug line to the native side, where it is rendered on screen.
///
/// Java signature: `private external fun nativeUsbLog(msg: String)`
/// on `dev.dioxus.main.MainActivity`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeUsbLog<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
    msg: JString<'a>,
) {
    env.with_env(|env| -> Result<_, jni::errors::Error> {
        if let Ok(jvm) = env.get_java_vm() {
            let _ = JVM.set(jvm);
        }
        if let Ok(s) = msg.try_to_string(&env) {
            let _ = EVENT_CHANNEL.tx.send(UsbEvent::Log(s));
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}
