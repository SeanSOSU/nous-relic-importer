package com.starrailsim.importer;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.LayerDrawable;
import android.net.Uri;
import android.net.VpnService;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.view.View;
import android.widget.Button;
import android.widget.TextView;

import java.util.List;

/**
 * One screen: Start turns on the app's VPN for the game and opens it; the account loads as the player logs in,
 * and reaches StarRailSim open on this phone, or another device through the share sheet.
 */
public final class MainActivity extends Activity {
    private static final Uri SITE = Uri.parse("https://starrailsim.com/import?sync=1");
    private static final int START = 1;

    private final Handler handler = new Handler(Looper.getMainLooper());
    /** Set when Start cannot go ahead, shown instead of the engine's status until the next Start. */
    private String problem;
    /** What the screen shows now, so a poll that changes nothing redraws nothing. */
    private String shown;
    /** The game, to open once the VPN is up: Android resets an app's connections as a VPN takes them over. */
    private Intent game;
    private long capturingSince;
    private final GradientDrawable ring = new GradientDrawable();
    private TextView headline, detail;
    private Button primary, share;

    private final Runnable refresh = new Runnable() {
        @Override
        public void run() {
            Status status = Status.read();
            show(status);
            openGameOnceCapturing(status);
            handler.postDelayed(this, 500);
        }
    };

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        setContentView(R.layout.activity_main);
        View disc = findViewById(R.id.disc);
        ring.setShape(GradientDrawable.OVAL);
        GradientDrawable centre = new GradientDrawable();
        centre.setShape(GradientDrawable.OVAL);
        centre.setColor(0xffffffff);
        LayerDrawable dot = new LayerDrawable(new GradientDrawable[] {ring, centre});
        int inset = disc.getLayoutParams().width * 11 / 32;
        dot.setLayerInset(1, inset, inset, inset, inset);
        disc.setBackground(dot);
        headline = findViewById(R.id.headline);
        detail = findViewById(R.id.detail);
        primary = findViewById(R.id.primary);
        share = findViewById(R.id.share);
        Engine.start(getFilesDir().getPath());
        primary.setOnClickListener(v -> {
            if (Status.read().capturing) stop();
            else start();
        });
        findViewById(R.id.open_site).setOnClickListener(v -> startActivity(new Intent(Intent.ACTION_VIEW, SITE)));
        share.setOnClickListener(v -> share());
    }

    @Override
    protected void onResume() {
        super.onResume();
        handler.post(refresh);
    }

    @Override
    protected void onPause() {
        handler.removeCallbacks(refresh);
        super.onPause();
    }

    private void start() {
        problem = null;
        if (Game.installed(getPackageManager()).isEmpty()) {
            problem = getString(R.string.needs_game);
            return;
        }
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS}, 0);
        }
        // Android's "Connection request", the first time only.
        Intent consent = VpnService.prepare(this);
        if (consent != null) startActivityForResult(consent, START);
        else onActivityResult(START, RESULT_OK, null);
    }

    private void stop() {
        startService(new Intent(this, CaptureService.class).setAction(CaptureService.ACTION_STOP));
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != START) return;
        if (result != RESULT_OK) {
            problem = getString(R.string.vpn_refused);
            return;
        }
        startForegroundService(new Intent(this, CaptureService.class));
        List<String> games = Game.installed(getPackageManager());
        game = games.isEmpty() ? null : getPackageManager().getLaunchIntentForPackage(games.get(0));
        capturingSince = 0;
    }

    /** Opens the game a second after the capture starts, by when Android has moved the game's traffic onto it. */
    private void openGameOnceCapturing(Status status) {
        if (game == null || !status.capturing) return;
        long now = SystemClock.elapsedRealtime();
        if (capturingSince == 0) capturingSince = now;
        if (now - capturingSince < 1000) return;
        startActivity(game);
        game = null;
    }

    private void share() {
        Intent send = new Intent(Intent.ACTION_SEND)
                .setType("application/json")
                .putExtra(Intent.EXTRA_STREAM, AccountProvider.ACCOUNT)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        startActivity(Intent.createChooser(send, getString(R.string.share)));
    }

    private void show(Status status) {
        int tone = problem != null ? Status.WARN : status.tone();
        String head = problem != null ? getString(R.string.not_ready) : status.headline(this);
        String text = problem != null ? problem : status.detail(this);
        String screen = tone + "\n" + head + "\n" + text + "\n" + status.capturing + "\n" + status.hasAccount;
        if (screen.equals(shown)) return;
        shown = screen;
        ring.setColor(tone);
        headline.setText(head);
        detail.setText(text);
        detail.setVisibility(text.isEmpty() ? View.GONE : View.VISIBLE);
        primary.setText(status.capturing ? R.string.stop : R.string.start);
        share.setVisibility(status.hasAccount ? View.VISIBLE : View.GONE);
    }
}
