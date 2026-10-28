package com.starrailsim.importer;

/** The Rust engine (android/engine): the desktop app's importer, with the app's own VPN as its packet source. */
final class Engine {
    static {
        System.loadLibrary("nri");
    }

    private Engine() {}

    /** Starts the engine once per process, keeping the account in {@code dataDir}: 0, or -1 if it could not. */
    static native int start(String dataDir);

    /** Relays and reads the game's traffic through the VPN interface {@code fd}, which the engine now owns. */
    static native void capture(int fd);

    /** Ends the capture; the VPN interface closes with it. */
    static native void stop();

    /** What the engine is doing, as JSON: see {@code status} in android/engine/src/lib.rs. */
    static native String status();
}
