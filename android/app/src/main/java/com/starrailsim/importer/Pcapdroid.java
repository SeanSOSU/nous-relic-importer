package com.starrailsim.importer;

import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;

import java.util.ArrayList;
import java.util.List;

/**
 * PCAPdroid's control API (https://github.com/emanuele-f/PCAPdroid/blob/master/docs/app_api.md): it captures one
 * app's traffic through Android's VPN service, with no root, and streams it to this app as it goes.
 */
final class Pcapdroid {
    static final String PACKAGE = "com.emanuelef.remote_capture";
    static final Uri STORE = Uri.parse("https://play.google.com/store/apps/details?id=" + PACKAGE);
    /** Honkai: Star Rail: the global release, then the two Chinese ones. */
    static final String[] GAMES = {"com.HoYoverse.hkrpgoversea", "com.miHoYo.hkrpg", "com.miHoYo.hkrpg.bilibili"};

    private Pcapdroid() {}

    static boolean installed(PackageManager packages) {
        return isInstalled(packages, PACKAGE);
    }

    static List<String> installedGames(PackageManager packages) {
        List<String> games = new ArrayList<>();
        for (String game : GAMES) {
            if (isInstalled(packages, game)) games.add(game);
        }
        return games;
    }

    private static boolean isInstalled(PackageManager packages, String name) {
        try {
            packages.getPackageInfo(name, 0);
            return true;
        } catch (PackageManager.NameNotFoundException e) {
            return false;
        }
    }

    /** One API call; PCAPdroid asks the player's consent the first time, so start it for a result. */
    static Intent control(String action) {
        Intent intent = new Intent(Intent.ACTION_VIEW);
        intent.setClassName(PACKAGE, PACKAGE + ".activities.CaptureCtrl");
        intent.putExtra("action", action);
        return intent;
    }

    /** Captures only the game, as a PCAP stream to this app's collector on {@code port}. */
    static Intent start(int port, List<String> games) {
        Intent intent = control("start");
        intent.putExtra("pcap_dump_mode", "tcp_exporter");
        intent.putExtra("collector_host", "127.0.0.1");
        intent.putExtra("collector_ip_address", "127.0.0.1");
        intent.putExtra("collector_port", String.valueOf(port));
        intent.putExtra("app_filter", String.join(",", games));
        return intent;
    }
}
