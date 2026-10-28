package com.starrailsim.importer;

import android.content.pm.PackageManager;

import java.util.ArrayList;
import java.util.List;

/** Honkai: Star Rail on this phone: the only app the VPN carries. */
final class Game {
    /** The global release, then the two Chinese ones. */
    static final String[] PACKAGES = {"com.HoYoverse.hkrpgoversea", "com.miHoYo.hkrpg", "com.miHoYo.hkrpg.bilibili"};

    private Game() {}

    static List<String> installed(PackageManager packages) {
        List<String> games = new ArrayList<>();
        for (String game : PACKAGES) {
            try {
                packages.getPackageInfo(game, 0);
                games.add(game);
            } catch (PackageManager.NameNotFoundException e) {
                // Not this release.
            }
        }
        return games;
    }
}
