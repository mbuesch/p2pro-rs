use jni::{
    JNIEnv,
    objects::{JObject, JString},
};
use std::{os::fd::RawFd, sync::LazyLock};
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
    _env: JNIEnv<'local>,
    _this: JObject<'local>,
    fd: i32,
    vendor_id: i32,
    product_id: i32,
) {
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
    if let Ok(java_str) = env.get_string(&msg) {
        let _ = EVENT_CHANNEL.tx.send(UsbEvent::Log(java_str.into()));
    }
}
