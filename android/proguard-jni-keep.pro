# Keep the MainActivity static methods that Rust calls by name via JNI.
# R8 (isMinifyEnabled = true in release builds) has no visibility into those native call sites,
# so it would otherwise freely rename/remove these methods, causing GetStaticMethodID to
# return NULL and the JNI call to fail.

-keepclassmembers class dev.dioxus.main.MainActivity {
    public static void saveFileBytes(java.lang.String, byte[]);
    public static void onNativeUsbSessionEnded(long);
}
