use anyhow::{self as ah, Context as _, format_err as err};
use jni::{
    Env, EnvUnowned, JavaVM,
    errors::ThrowRuntimeExAndDefault,
    jni_sig, jni_str,
    objects::{JClass, JObject, JString, JValue},
    refs::Global,
};
use std::{
    os::fd::RawFd,
    sync::{LazyLock, Mutex as StdMutex, OnceLock},
};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

/// An event handed over from the Android `MainActivity`.
pub enum UsbEvent {
    /// `UsbManager.openDevice()` succeeded.
    /// Carries `(fd, vendor_id, product_id, session_token)` of an opened, permission-granted USB device.
    DeviceReady(RawFd, u16, u16, i64),
    /// A human-readable log to be shown on screen.
    Log(String),
}

struct EventChannel {
    tx: mpsc::Sender<UsbEvent>,
    rx: AsyncMutex<mpsc::Receiver<UsbEvent>>,
}

static EVENT_CHANNEL: LazyLock<EventChannel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel(128);
    EventChannel {
        tx,
        rx: AsyncMutex::new(rx),
    }
});

static JVM: OnceLock<JavaVM> = OnceLock::new();
static MAIN_ACTIVITY_CLASS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Caches the `MainActivity` class.
fn cache_main_activity_class(env: &mut Env<'_>) {
    if MAIN_ACTIVITY_CLASS.get().is_none()
        && let Ok(cls) = env.find_class(jni_str!("dev/dioxus/main/MainActivity"))
        && let Ok(global) = env.new_global_ref(cls)
    {
        let _ = MAIN_ACTIVITY_CLASS.set(global);
    }
}

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
    let _ = jvm.attach_current_thread(|env| -> ah::Result<()> {
        if let Some(cls) = MAIN_ACTIVITY_CLASS.get() {
            let args = [JValue::Long(session_token)];
            if let Err(e) = env.call_static_method(
                cls,
                jni_str!("onNativeUsbSessionEnded"),
                jni_sig!((token: long) -> void),
                &args,
            ) {
                log::error!("notify_session_ended: call_static_method failed: {e}");
                clear_pending_exception(env);
            }
        }
        Ok(())
    });
}

/// Opens the Android Storage Access Framework save dialog.
pub async fn save_file(filename: &str, bytes: &[u8]) -> ah::Result<()> {
    let jvm = JVM.get().context("JVM not initialized")?;
    let cls = MAIN_ACTIVITY_CLASS
        .get()
        .context("MainActivity class not cached yet")?;
    jvm.attach_current_thread(|env| -> ah::Result<()> {
        let filename_jstring = env.new_string(filename)?;
        let bytes_array = env.byte_array_from_slice(bytes)?;
        let args = [
            JValue::Object(&filename_jstring),
            JValue::Object(&bytes_array),
        ];
        env.call_static_method(
            cls,
            jni_str!("saveFileBytes"),
            jni_sig!((filename: java.lang.String, bytes: byte[]) -> void),
            &args,
        )?;
        Ok(())
    })
}

/// A pending video-file pick: the Kotlin side delivers the result fd here.
static VIDEO_PICK: StdMutex<Option<oneshot::Sender<i32>>> = StdMutex::new(None);

/// Opens the Android SAF "create document" dialog for the video recording.
///
/// Returns the writable (and seekable) file descriptor of the picked
/// document, or `None` if the user cancelled.
pub async fn pick_video_file(filename: &str) -> ah::Result<Option<RawFd>> {
    let (tx, rx) = oneshot::channel();
    *VIDEO_PICK.lock().expect("Lock poisoned") = Some(tx);

    let call = (|| -> ah::Result<()> {
        let jvm = JVM.get().context("JVM not initialized")?;
        let cls = MAIN_ACTIVITY_CLASS
            .get()
            .context("MainActivity class not cached yet")?;
        jvm.attach_current_thread(|env| -> ah::Result<()> {
            let filename_jstring = env.new_string(filename)?;
            let args = [JValue::Object(&filename_jstring)];
            env.call_static_method(
                cls,
                jni_str!("requestVideoFile"),
                jni_sig!((filename: java.lang.String) -> void),
                &args,
            )?;
            Ok(())
        })
    })();
    if let Err(e) = call {
        VIDEO_PICK.lock().expect("Lock poisoned").take();
        return Err(e);
    }

    match rx.await {
        Ok(fd) if fd >= 0 => Ok(Some(fd)),
        Ok(_) => Ok(None),
        Err(_) => Err(err!("Video file picker closed unexpectedly")),
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
        cache_main_activity_class(env);
        if let Err(e) = EVENT_CHANNEL.tx.try_send(UsbEvent::DeviceReady(
            fd as RawFd,
            vendor_id as u16,
            product_id as u16,
            token,
        )) {
            eprintln!("Failed to send USB device ready event: {:?}", e);
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

/// Called from Kotlin (`MainActivity.nativeVideoFileReady`) when the SAF
/// "create document" dialog for the video recording has closed. `fd` is a
/// detached, writable file descriptor for the picked document, or -1 if the
/// user cancelled.
///
/// Java signature: `private external fun nativeVideoFileReady(fd: Int)`
/// on `dev.dioxus.main.MainActivity`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_dioxus_main_MainActivity_nativeVideoFileReady<'a>(
    mut env: EnvUnowned<'a>,
    _this: JObject<'a>,
    fd: i32,
) {
    env.with_env(|env| -> Result<_, jni::errors::Error> {
        if let Ok(jvm) = env.get_java_vm() {
            let _ = JVM.set(jvm);
        }
        cache_main_activity_class(env);
        let tx = VIDEO_PICK.lock().expect("Lock poisoned").take();
        if let Some(tx) = tx {
            let _ = tx.send(fd);
        } else if fd >= 0 {
            // No waiting picker (the request was aborted);
            // don't leak the descriptor.
            unsafe { libc::close(fd) };
        }
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
        cache_main_activity_class(env);
        if let Ok(s) = msg.try_to_string(&env) {
            if let Err(e) = EVENT_CHANNEL.tx.try_send(UsbEvent::Log(s)) {
                eprintln!("Failed to send USB log event: {:?}", e);
            }
        }
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}
