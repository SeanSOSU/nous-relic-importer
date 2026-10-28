package com.starrailsim.importer;

/** The Rust engine (android/engine): the desktop app's importer, with PCAPdroid as its packet source. */
final class Engine {
    static {
        System.loadLibrary("nri");
    }

    private Engine() {}

    /** Starts the engine once per process, keeping the account in {@code dataDir}; the port PCAPdroid sends to, or -1. */
    static native int start(String dataDir);

    /** What the engine is doing, as JSON: see {@code status} in android/engine/src/lib.rs. */
    static native String status();
}
