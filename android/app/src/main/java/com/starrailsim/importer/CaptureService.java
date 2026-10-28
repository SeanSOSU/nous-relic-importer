package com.starrailsim.importer;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ServiceInfo;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.VpnService;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.ParcelFileDescriptor;

import java.net.InetAddress;
import java.util.List;

/**
 * The app's VPN, for the game only: Android routes the game's traffic here, the engine passes it on to where it
 * was going and reads the login on the way. Runs in the foreground, with a notification to stop it, until Stop,
 * or until Android takes the VPN away.
 */
public final class CaptureService extends VpnService {
    static final String ACTION_STOP = "com.starrailsim.importer.STOP";
    private static final String CHANNEL = "import";
    private static final int ID = 1;

    private final Handler handler = new Handler(Looper.getMainLooper());

    private final Runnable refresh = new Runnable() {
        @Override
        public void run() {
            Status status = Status.read();
            if (!status.capturing) {
                stop();
                return;
            }
            getSystemService(NotificationManager.class).notify(ID, notification(status));
            handler.postDelayed(this, 2000);
        }
    };

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            stop();
            return START_NOT_STICKY;
        }
        Engine.start(getFilesDir().getPath());
        NotificationManager notifications = getSystemService(NotificationManager.class);
        notifications.createNotificationChannel(new NotificationChannel(CHANNEL, getString(R.string.channel), NotificationManager.IMPORTANCE_LOW));
        Notification notification = notification(Status.read());
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        } else {
            startForeground(ID, notification);
        }
        ParcelFileDescriptor vpn = establish();
        if (vpn == null) {
            stop();
            return START_NOT_STICKY;
        }
        Engine.capture(vpn.detachFd());
        handler.removeCallbacks(refresh);
        handler.postDelayed(refresh, 2000);
        return START_NOT_STICKY;
    }

    /** The VPN interface: every route, but only the game's traffic; null if Android would not make it. */
    private ParcelFileDescriptor establish() {
        List<String> games = Game.installed(getPackageManager());
        if (games.isEmpty()) return null;
        Builder builder = new Builder()
                .setSession(getString(R.string.app_name))
                .setMtu(1500)
                .setBlocking(false)
                .addAddress("10.111.0.1", 32)
                .addAddress("fd6e:7269::1", 128)
                .addRoute("0.0.0.0", 0)
                .addRoute("::", 0)
                .setConfigureIntent(PendingIntent.getActivity(this, 0, new Intent(this, MainActivity.class), PendingIntent.FLAG_IMMUTABLE));
        // The phone's own DNS servers: the game looks names up where it always does, through the relay.
        for (InetAddress server : systemDns()) builder.addDnsServer(server);
        try {
            for (String game : games) builder.addAllowedApplication(game);
        } catch (PackageManager.NameNotFoundException e) {
            return null;
        }
        return builder.establish();
    }

    private List<InetAddress> systemDns() {
        ConnectivityManager connectivity = getSystemService(ConnectivityManager.class);
        Network network = connectivity.getActiveNetwork();
        LinkProperties link = network == null ? null : connectivity.getLinkProperties(network);
        return link == null ? List.of() : link.getDnsServers();
    }

    private Notification notification(Status status) {
        Intent open = new Intent(this, MainActivity.class).setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        Intent stop = new Intent(this, CaptureService.class).setAction(ACTION_STOP);
        int flags = PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT;
        return new Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_notify)
                .setColor(status.tone())
                .setContentTitle(status.headline(this))
                .setContentText(status.detail(this))
                .setContentIntent(PendingIntent.getActivity(this, 0, open, flags))
                .addAction(new Notification.Action.Builder(null, getString(R.string.stop), PendingIntent.getService(this, 1, stop, flags)).build())
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .build();
    }

    private void stop() {
        handler.removeCallbacks(refresh);
        Engine.stop();
        stopForeground(STOP_FOREGROUND_REMOVE);
        stopSelf();
    }

    @Override
    public void onRevoke() {
        stop();
    }

    @Override
    public void onDestroy() {
        handler.removeCallbacks(refresh);
        Engine.stop();
        super.onDestroy();
    }
}
