#!/usr/bin/env bash
# Builds Argus-Node.apk without Gradle/Android Studio (how the shipped APK was made).
# Usage: TOOLS=/workspace/work/apk AAPT2="qemu-x86_64-static /workspace/work/apk/aapt2" ./build-apk-nogradle.sh
# Needs: aapt2 (Google maven), kotlinc 2.0.x, r8.jar (d8), android-35/android.jar, zipalign, apksigner, JDK 17+.
set -euo pipefail
T=${TOOLS:-./tools}; AAPT2=${AAPT2:-$T/aapt2}; M=app/src/main; B=build-nogradle; rm -rf $B; mkdir -p $B/classes $B/lib/arm64-v8a
sed 's|<manifest xmlns:android="http://schemas.android.com/apk/res/android">|<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.harkcluster.node">|' $M/AndroidManifest.xml > $B/AndroidManifest.xml
$AAPT2 compile --dir $M/res -o $B/res.zip
$AAPT2 link -o $B/base.apk -I $T/android.jar --manifest $B/AndroidManifest.xml --min-sdk-version 28 --target-sdk-version 35 --version-code 3 --version-name 0.2 $B/res.zip
$T/kotlinc/bin/kotlinc $M/java/com/harkcluster/node/*.kt -no-stdlib -cp $T/android.jar:$T/kotlinc/lib/kotlin-stdlib.jar -jvm-target 17 -d $B/classes
(cd $B/classes && jar cf ../classes.jar .)
java -cp $T/r8.jar com.android.tools.r8.D8 --release --min-api 28 --lib $T/android.jar --output $B $B/classes.jar $T/kotlinc/lib/kotlin-stdlib.jar
cp $M/jniLibs/arm64-v8a/*.so $B/lib/arm64-v8a/   # librpc_server{,_dotprod,_i8mm}.so
(cd $B && cp base.apk u.apk && zip -q u.apk classes.dex && zip -q -0 u.apk lib/arm64-v8a/*.so && zipalign -f -p 4 u.apk a.apk)
apksigner sign --ks argus-debug.jks --ks-pass pass:argus123 --out Argus-Node.apk $B/a.apk
echo "-> Argus-Node.apk"

# Quick repack (no aapt2): when only Kotlin changed, compile + d8, then copy every entry of the
# previous APK except META-INF/ and classes.dex, add the new classes.dex, zipalign -p 4, apksigner.
