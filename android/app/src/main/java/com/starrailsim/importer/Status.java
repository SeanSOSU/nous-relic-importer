package com.starrailsim.importer;

import android.content.Context;

import org.json.JSONException;
import org.json.JSONObject;

import java.text.NumberFormat;

/** The engine's status as the app words it, in the window and the notification alike: the desktop app's wording. */
final class Status {
    static final int IDLE = 0xff8a8f98, OK = 0xff199e70, WARN = 0xffd9a441;

    final String state;
    final String reason;
    final boolean capturing;
    final int pages;
    final int relics;
    final boolean fresh;
    final boolean hasAccount;

    private Status(JSONObject json) {
        state = json.optString("state", "stopped");
        reason = json.optString("detail", "");
        capturing = json.optBoolean("capturing");
        pages = json.optInt("pages");
        relics = json.optInt("relics");
        fresh = json.optBoolean("fresh");
        hasAccount = !json.isNull("accountFile");
    }

    static Status read() {
        try {
            return new Status(new JSONObject(Engine.status()));
        } catch (JSONException e) {
            return new Status(new JSONObject());
        }
    }

    private boolean ready() {
        return state.equals("starting") || state.equals("stopped") || (state.equals("waiting") && !capturing);
    }

    String headline(Context c) {
        if (ready()) return c.getString(R.string.ready);
        switch (state) {
            case "synced": return c.getString(R.string.synced_relics, NumberFormat.getIntegerInstance().format(relics));
            case "waiting": return c.getString(R.string.waiting);
            case "gameFound": return c.getString(R.string.game_found);
            case "loggingIn": return c.getString(R.string.logging_in);
            case "needsRelogin": return c.getString(R.string.relog);
            default: return c.getString(R.string.not_working);
        }
    }

    String detail(Context c) {
        if (ready()) return c.getString(R.string.ready_detail);
        switch (state) {
            case "synced":
                if (!fresh) return c.getString(R.string.synced_saved);
                if (!capturing) return c.getString(R.string.synced_capture_off);
                return c.getString(pages == 0 ? R.string.synced_open_site : R.string.synced_up_to_date);
            case "waiting": return c.getString(R.string.waiting_detail);
            case "gameFound": return c.getString(R.string.game_found_detail);
            case "loggingIn": return "";
            case "needsRelogin": return c.getString(R.string.relog_detail);
            default: return c.getString(R.string.not_working_detail, reason);
        }
    }

    int tone() {
        if (state.equals("synced")) return OK;
        if (state.equals("needsRelogin") || state.equals("failed")) return WARN;
        return IDLE;
    }
}
