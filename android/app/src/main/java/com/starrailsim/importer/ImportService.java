package com.starrailsim.importer;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.SystemClock;

/**
 * Keeps the process, and with it the engine reading PCAPdroid's capture, running while the game is in front.
 * It ends itself once the capture does, or if none arrives.
 */
public final class ImportService extends Service {
    private static final String CHANNEL = "import";
    private static final int ID = 1;
    private static final long NO_CAPTURE_MS = 120_000;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private long started;
    private boolean seenCapture;

    private final Runnable refresh = new Runnable() {
        @Override
        public void run() {
            Status status = Status.read();
            seenCapture |= status.capturing;
            boolean over = seenCapture ? !status.capturing : SystemClock.elapsedRealtime() - started > NO_CAPTURE_MS;
            if (over) {
                stopSelf();
                return;
            }
            getSystemService(NotificationManager.class).notify(ID, notification(status));
            handler.postDelayed(this, 2000);
        }
    };

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        Engine.start(getFilesDir().getPath());
        NotificationManager notifications = getSystemService(NotificationManager.class);
        notifications.createNotificationChannel(new NotificationChannel(CHANNEL, getString(R.string.channel), NotificationManager.IMPORTANCE_LOW));
        Notification notification = notification(Status.read());
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        } else {
            startForeground(ID, notification);
        }
        started = SystemClock.elapsedRealtime();
        seenCapture = false;
        handler.removeCallbacks(refresh);
        handler.postDelayed(refresh, 2000);
        return START_NOT_STICKY;
    }

    private Notification notification(Status status) {
        Intent open = new Intent(this, MainActivity.class).setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        Intent stop = new Intent(this, MainActivity.class).setAction(MainActivity.ACTION_STOP).setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        int flags = PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT;
        return new Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_notify)
                .setColor(status.tone())
                .setContentTitle(status.headline(this))
                .setContentText(status.detail(this))
                .setContentIntent(PendingIntent.getActivity(this, 0, open, flags))
                .addAction(new Notification.Action.Builder(null, getString(R.string.stop), PendingIntent.getActivity(this, 1, stop, flags)).build())
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .build();
    }

    @Override
    public void onDestroy() {
        handler.removeCallbacks(refresh);
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
