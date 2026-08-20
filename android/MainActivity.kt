package dev.dioxus.main

import android.Manifest
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Bundle
import android.util.Log
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import java.lang.ref.WeakReference
import kotlin.jvm.JvmStatic

typealias BuildConfig = ch.bues.p2pro.BuildConfig

class MainActivity : WryActivity() {
    private val usbManager: UsbManager by lazy {
        getSystemService(Context.USB_SERVICE) as UsbManager
    }

    private var openConnection: UsbDeviceConnection? = null
    private var openDeviceName: String? = null
    private var permissionRequestedFor: String? = null
    private var pendingAfterCameraPermission: UsbDevice? = null
    private var nativeSessionActive: Boolean = false
    private var pendingDetach: Boolean = false

    private val cameraPermissionLauncher: ActivityResultLauncher<String> =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            logUsb("CAMERA runtime permission ${if (granted) "GRANTED" else "DENIED"}")
            val device = pendingAfterCameraPermission
            pendingAfterCameraPermission = null
            if (device != null) {
                if (granted) {
                    requestPermission(device)
                } else {
                    logUsb("Cannot request USB permission for ${device.deviceName} without CAMERA permission")
                }
            }
        }

    private fun hasCameraPermission(): Boolean =
        checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

    private val usbReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                ACTION_USB_PERMISSION -> {
                    val device = usbDeviceExtra(intent) ?: return
                    // Dump all extras - on failure paths Android omits EXTRA_PERMISSION_GRANTED entirely.
                    val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                    val extras = intent.extras?.keySet()?.joinToString(",") ?: "(none)"
                    if (granted) {
                        logUsb("USB permission GRANTED for ${describe(device)}")
                        openDevice(device)
                    } else {
                        logUsb("USB permission DENIED for ${describe(device)} [extras=$extras]")
                        logUsb("Hint: if no dialog appeared, check that CAMERA permission is granted in system settings")
                    }
                    permissionRequestedFor = null
                }
                UsbManager.ACTION_USB_DEVICE_ATTACHED -> {
                    val device = usbDeviceExtra(intent) ?: return
                    logUsb("USB attached: ${describe(device)}")
                    handleDevice(device)
                }
                UsbManager.ACTION_USB_DEVICE_DETACHED -> {
                    val device = usbDeviceExtra(intent) ?: return
                    logUsb("USB detached: ${describe(device)}")
                    if (device.deviceName == openDeviceName) {
                        if (nativeSessionActive) {
                            logUsb("Native session still running; holding fd open until it ends")
                            pendingDetach = true
                        } else {
                            logUsb("No native session active; closing connection now")
                            closeOpenConnection()
                        }
                    }
                }
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        currentActivity = WeakReference(this)
        val filter = IntentFilter().apply {
            addAction(ACTION_USB_PERMISSION)
            addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(usbReceiver, filter, Context.RECEIVER_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            registerReceiver(usbReceiver, filter)
        }
        handleLaunchIntent(intent, "onCreate")
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleLaunchIntent(intent, "onNewIntent")
    }

    override fun onResume() {
        super.onResume()
        scanUsbBus("onResume")
    }

    override fun onDestroy() {
        currentActivity?.clear()
        currentActivity = null
        unregisterReceiver(usbReceiver)
        super.onDestroy()
    }

    private fun handleLaunchIntent(intent: Intent?, source: String) {
        if (intent?.action == UsbManager.ACTION_USB_DEVICE_ATTACHED) {
            val device = usbDeviceExtra(intent)
            if (device == null) {
                logUsb("Launched by USB attach ($source), but the intent has no device extra")
                return
            }
            logUsb("Launched by USB attach ($source): ${describe(device)}")
            handleDevice(device)
        }
    }

    private fun scanUsbBus(source: String) {
        val devices = try {
            usbManager.deviceList
        } catch (e: Exception) {
            logUsb("USB scan ($source) failed: $e")
            return
        }
        logUsb("USB scan ($source): ${devices.size} device(s) on the bus")
        for (device in devices.values) {
            logUsb("  ${describe(device)}")
        }
        val p2pro = devices.values.firstOrNull { isP2Pro(it) }
        if (p2pro == null) {
            logUsb("  no P2Pro (%04x:%04x) found".format(P2PRO_VENDOR_ID, P2PRO_PRODUCT_ID))
        } else {
            handleDevice(p2pro)
        }
    }

    private fun handleDevice(device: UsbDevice) {
        if (!isP2Pro(device)) {
            return
        }
        when {
            openDeviceName == device.deviceName ->
                logUsb("P2Pro ${device.deviceName} is already open; ignoring")
            usbManager.hasPermission(device) -> {
                logUsb("P2Pro found and permission already granted")
                openDevice(device)
            }
            !hasCameraPermission() -> {
                logUsb("P2Pro found; requesting CAMERA runtime permission first (required for USB on Android 14+)")
                pendingAfterCameraPermission = device
                cameraPermissionLauncher.launch(Manifest.permission.CAMERA)
            }
            else -> requestPermission(device)
        }
    }

    private fun isP2Pro(device: UsbDevice): Boolean =
        device.vendorId == P2PRO_VENDOR_ID && device.productId == P2PRO_PRODUCT_ID

    private fun requestPermission(device: UsbDevice) {
        if (permissionRequestedFor == device.deviceName) {
            return // Dialog already on its way; don't stack requests.
        }
        permissionRequestedFor = device.deviceName

        logUsb("Requesting USB permission for ${device.deviceName} - grant the popup!")
        val intent = Intent(ACTION_USB_PERMISSION).setPackage(packageName)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0
        val pending = PendingIntent.getBroadcast(this, 0, intent, flags)
        try {
            usbManager.requestPermission(device, pending)
            logUsb("requestPermission() returned; waiting for user response ...")
        } catch (e: Exception) {
            permissionRequestedFor = null
            logUsb("requestPermission(${device.deviceName}) threw: $e")
        }
    }

    private fun openDevice(device: UsbDevice) {
        if (openConnection != null) {
            logUsb("openDevice(${device.deviceName}) called while a connection already exists; ignoring")
            return
        }
        val connection = try {
            usbManager.openDevice(device)
        } catch (e: Exception) {
            logUsb("openDevice(${device.deviceName}) threw: $e")
            return
        }
        if (connection == null) {
            logUsb("openDevice(${device.deviceName}) returned null")
            return
        }
        openConnection = connection
        openDeviceName = device.deviceName
        nativeSessionActive = true
        pendingDetach = false
        val fd = connection.fileDescriptor
        logUsb("Opened ${device.deviceName}: fd=$fd, handing it to the native side")
        nativeUsbDeviceReady(fd, device.vendorId, device.productId)
    }

    private fun closeOpenConnection() {
        openConnection?.close()
        openConnection = null
        openDeviceName = null
        nativeSessionActive = false
        pendingDetach = false
        permissionRequestedFor = null
    }

    private fun describe(device: UsbDevice): String {
        val id = "%04x:%04x".format(device.vendorId, device.productId)
        val name = runCatching { device.productName }.getOrNull() ?: "?"
        val perm = runCatching { usbManager.hasPermission(device) }.getOrDefault(false)
        return "${device.deviceName} [$id] \"$name\" perm=$perm"
    }

    private fun usbDeviceExtra(intent: Intent): UsbDevice? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
        }

    private fun logUsb(msg: String) {
        Log.d(TAG, msg)
        runCatching { nativeUsbLog(msg) }
    }

    /** Implemented in Rust, see src/camera/android/jni_bridge.rs. */
    private external fun nativeUsbDeviceReady(fd: Int, vendorId: Int, productId: Int)

    /** Implemented in Rust, see src/camera/android/jni_bridge.rs. */
    private external fun nativeUsbLog(msg: String)

    private companion object {
        const val TAG = "P2ProUsb"
        const val ACTION_USB_PERMISSION = "dev.dioxus.main.USB_PERMISSION"
        const val P2PRO_VENDOR_ID = 0x0bda
        const val P2PRO_PRODUCT_ID = 0x5830

        private var currentActivity: WeakReference<MainActivity>? = null

        /** Called from Rust once the native USB session has ended. */
        @JvmStatic
        fun onNativeUsbSessionEnded() {
            currentActivity?.get()?.let { activity ->
                activity.runOnUiThread {
                    activity.logUsb("Native USB session ended")
                    if (activity.pendingDetach) {
                        activity.logUsb("Processing pending detach: closing USB connection")
                    }
                    activity.closeOpenConnection()
                }
            }
        }
    }
}
