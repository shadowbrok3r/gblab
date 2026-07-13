package com.kingsofalchemy.gblab;

import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothGatt;
import android.bluetooth.BluetoothGattCallback;
import android.bluetooth.BluetoothGattCharacteristic;
import android.bluetooth.BluetoothGattDescriptor;
import android.bluetooth.BluetoothGattService;
import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothProfile;
import android.content.Context;
import android.content.pm.PackageManager;
import android.os.Build;
import java.util.UUID;

/**
 * BLE link to the GBLab Pad (ESP32-H2). The native side calls start()/stop()
 * and polls the static fields; the pad uses a fixed static random address, so
 * no scanning (and no location permission) is needed.
 */
public final class BleController {
    public static final int IDLE = 0;
    public static final int NEED_PERMISSION = 1;
    public static final int CONNECTING = 2;
    public static final int CONNECTED = 3;
    public static final int FAILED = 4;

    public static volatile int state = IDLE;
    public static volatile int buttons = 0;
    public static volatile String detail = "";

    // Must match the firmware's Address::random bytes (reversed for display).
    private static final String PAD_MAC = "FF:62:4C:42:47:FF";
    private static final UUID SERVICE = UUID.fromString("8f7a2d43-1e5b-4c9a-9d0e-5c33a1b0f001");
    private static final UUID BUTTONS = UUID.fromString("8f7a2d43-1e5b-4c9a-9d0e-5c33a1b0f002");
    private static final UUID CCCD = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb");

    private static BluetoothGatt gatt;
    private static boolean permissionRequested;

    private BleController() {}

    public static synchronized void start(Context context) {
        if (state == CONNECTING || state == CONNECTED) {
            return;
        }
        if (!(context instanceof Activity)) {
            state = FAILED;
            detail = "internal: no activity";
            return;
        }
        Activity activity = (Activity) context;
        if (Build.VERSION.SDK_INT < 33) {
            state = FAILED;
            detail = "needs android 13+";
            return;
        }
        if (activity.checkSelfPermission("android.permission.BLUETOOTH_CONNECT")
                != PackageManager.PERMISSION_GRANTED) {
            state = NEED_PERMISSION;
            detail = "waiting for permission";
            if (!permissionRequested) {
                permissionRequested = true;
                activity.requestPermissions(
                        new String[] {"android.permission.BLUETOOTH_CONNECT"}, 71);
            }
            return;
        }
        BluetoothManager bm =
                (BluetoothManager) activity.getSystemService(Context.BLUETOOTH_SERVICE);
        BluetoothAdapter adapter = bm == null ? null : bm.getAdapter();
        if (adapter == null || !adapter.isEnabled()) {
            state = FAILED;
            detail = "bluetooth is off";
            return;
        }
        try {
            BluetoothDevice dev =
                    adapter.getRemoteLeDevice(PAD_MAC, BluetoothDevice.ADDRESS_TYPE_RANDOM);
            state = CONNECTING;
            detail = "connecting";
            gatt = dev.connectGatt(
                    activity.getApplicationContext(), false, callback,
                    BluetoothDevice.TRANSPORT_LE);
            if (gatt == null) {
                state = FAILED;
                detail = "bluetooth unavailable";
            }
        } catch (Exception e) {
            state = FAILED;
            detail = String.valueOf(e.getMessage());
        }
    }

    public static synchronized void stop() {
        if (gatt != null) {
            try {
                gatt.close();
            } catch (Exception ignored) {
            }
            gatt = null;
        }
        buttons = 0;
        permissionRequested = false;
        state = IDLE;
        detail = "";
    }

    private static final BluetoothGattCallback callback = new BluetoothGattCallback() {
        @Override
        public void onConnectionStateChange(BluetoothGatt g, int status, int newState) {
            if (newState == BluetoothProfile.STATE_CONNECTED) {
                detail = "discovering services";
                g.discoverServices();
            } else if (newState == BluetoothProfile.STATE_DISCONNECTED) {
                try {
                    g.close();
                } catch (Exception ignored) {
                }
                synchronized (BleController.class) {
                    if (gatt == g) {
                        gatt = null;
                    }
                }
                buttons = 0;
                if (state != IDLE) {
                    state = FAILED;
                    detail = "disconnected (status " + status + ")";
                }
            }
        }

        @Override
        public void onServicesDiscovered(BluetoothGatt g, int status) {
            BluetoothGattService svc = g.getService(SERVICE);
            BluetoothGattCharacteristic ch = svc == null ? null : svc.getCharacteristic(BUTTONS);
            if (ch == null) {
                detail = "pad service not found";
                g.disconnect();
                return;
            }
            g.setCharacteristicNotification(ch, true);
            BluetoothGattDescriptor d = ch.getDescriptor(CCCD);
            if (d == null) {
                state = CONNECTED;
                detail = "";
                return;
            }
            d.setValue(BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE);
            g.writeDescriptor(d);
        }

        @Override
        public void onDescriptorWrite(BluetoothGatt g, BluetoothGattDescriptor d, int status) {
            state = CONNECTED;
            detail = "";
        }

        // API 33+ callback.
        @Override
        public void onCharacteristicChanged(
                BluetoothGatt g, BluetoothGattCharacteristic ch, byte[] value) {
            if (value.length > 0) {
                buttons = value[0] & 0xFF;
            }
        }

        // Pre-33 callback; harmless duplicate on newer devices.
        @Override
        @SuppressWarnings("deprecation")
        public void onCharacteristicChanged(BluetoothGatt g, BluetoothGattCharacteristic ch) {
            byte[] v = ch.getValue();
            if (v != null && v.length > 0) {
                buttons = v[0] & 0xFF;
            }
        }
    };
}
