plugins {
    id("com.android.application")
}

android {
    namespace = "com.starrailsim.importer"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "com.starrailsim.importer"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// The engine (android/engine) for each ABI the app ships: phones run arm64, the emulator x86_64. cargo-ndk
// writes it to src/main/jniLibs, which the build packs as it is.
val ndk = androidComponents.sdkComponents.ndkDirectory
val rustEngine = tasks.register<Exec>("rustEngine") {
    workingDir = rootDir.parentFile
    doFirst { environment("ANDROID_NDK_HOME", ndk.get().asFile.path) }
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64", "--platform", "26",
        "-o", file("src/main/jniLibs").path,
        "build", "--release", "-p", "nri-android", "-j", providers.gradleProperty("cargoJobs").get(),
    )
}

tasks.named("preBuild") { dependsOn(rustEngine) }
