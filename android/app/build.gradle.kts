plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.harkcluster.node"
    compileSdk = 35
    defaultConfig {
        applicationId = "com.harkcluster.node"
        minSdk = 28
        targetSdk = 35
        versionCode = 5
        versionName = "0.3.1"
        ndk { abiFilters += listOf("arm64-v8a") }
    }
    buildTypes { release { isMinifyEnabled = false; signingConfig = signingConfigs.getByName("debug") } }
    // rpc-server ships as jniLibs/arm64-v8a/librpc_server.so. Android only lets apps exec files from
    // nativeLibraryDir (W^X since Android 10), and only extracts them there with legacy packaging.
    packaging { jniLibs { useLegacyPackaging = true } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
}
