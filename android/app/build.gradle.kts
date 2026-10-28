import java.util.Properties

plugins {
    id("com.android.application")
}

// The importer's version (the root Cargo.toml), so one release's APK and desktop builds match.
val importerVersion = rootDir.resolveSibling("Cargo.toml").readLines()
    .first { it.startsWith("version = ") }.substringAfter('"').substringBefore('"')

// Release builds are signed with the key -PnriSigning=<properties file> names (storeFile, storePassword, keyAlias,
// keyPassword); without it they come out unsigned.
val signing = providers.gradleProperty("nriSigning").orNull?.let { path ->
    Properties().apply { file(path).inputStream().use(::load) }
}

android {
    namespace = "com.starrailsim.importer"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "com.starrailsim.importer"
        minSdk = 26
        targetSdk = 36
        versionName = importerVersion
        versionCode = importerVersion.split('.').map(String::toInt).let { (major, minor, patch) -> major * 10000 + minor * 100 + patch }
    }

    signingConfigs {
        if (signing != null) {
            create("release") {
                storeFile = file(signing.getProperty("storeFile"))
                storePassword = signing.getProperty("storePassword")
                keyAlias = signing.getProperty("keyAlias")
                keyPassword = signing.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            if (signing != null) signingConfig = signingConfigs.getByName("release")
        }
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
