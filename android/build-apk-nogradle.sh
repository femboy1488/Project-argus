#!/usr/bin/env bash
# Builds Argus-Node.apk without Gradle/Android Studio (how the shipped APK was made).
# Usage: TOOLS=/workspace/work/apk AAPT2="qemu-x86_64-static /workspace/work/apk/aapt2" ./build-apk-nogradle.sh
# Needs: aapt2 (Google maven), kotlinc 2.0.x, r8.jar (d8), android-35/android.jar, zipalign, apksigner, JDK 17+.
# versionCode/versionName are read from app/build.gradle.kts.
# aarch64 sandbox (no root), as used for 0.3: wrappers in /workspace/tools/bin (put first in PATH):
#   java/jar  = openjdk-21-jdk-headless deb, dpkg -x into /workspace/tools/root (re-point conf symlinks /etc/... -> root/etc)
#   apksigner = apksigner+libapksig-java debs;  zipalign = zipalign + android-lib{base,cutils,log,utils,ziparchive,backtrace},
#               libzopfli1, 7zip debs (LD_LIBRARY_PATH incl. usr/lib/7zip)
#   qemu-x86_64-static = qemu-user-static deb run with -L /workspace/tools/x86root (amd64 libc6 + libgcc-s1 debs) for aapt2
set -euo pipefail
VC=$(sed -n 's/.*versionCode = \([0-9]*\).*/\1/p' app/build.gradle.kts); VN=$(sed -n 's/.*versionName = "\(.*\)".*/\1/p' app/build.gradle.kts)   # single source of truth
T=${TOOLS:-./tools}; AAPT2=${AAPT2:-$T/aapt2}; M=app/src/main; B=build-nogradle; rm -rf $B; mkdir -p $B/classes $B/lib/arm64-v8a
sed 's|<manifest xmlns:android="http://schemas.android.com/apk/res/android">|<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.harkcluster.node">|' $M/AndroidManifest.xml > $B/AndroidManifest.xml
$AAPT2 compile --dir $M/res -o $B/res.zip
$AAPT2 link -o $B/base.apk -I $T/android.jar --manifest $B/AndroidManifest.xml --min-sdk-version 28 --target-sdk-version 35 --version-code $VC --version-name $VN $B/res.zip
$T/kotlinc/bin/kotlinc $M/java/com/harkcluster/node/*.kt -no-stdlib -cp $T/android.jar:$T/kotlinc/lib/kotlin-stdlib.jar -jvm-target 17 -d $B/classes
(cd $B/classes && jar cf ../classes.jar .)
java -cp $T/r8.jar com.android.tools.r8.D8 --release --min-api 28 --lib $T/android.jar --output $B $B/classes.jar $T/kotlinc/lib/kotlin-stdlib.jar
cp $M/jniLibs/arm64-v8a/*.so $B/lib/arm64-v8a/   # rpc_server{,_dotprod,_i8mm}, llama_server, whisper_server, kiwix_serve, argus_bench{,_dotprod}; deflated (manifest has extractNativeLibs=true)
(cd $B && cp base.apk u.apk && zip -q u.apk classes.dex && zip -q -9 u.apk lib/arm64-v8a/*.so && zipalign -f -p 4 u.apk a.apk)
apksigner sign --ks argus-debug.jks --ks-pass pass:argus123 --out Argus-Node.apk $B/a.apk
echo "-> Argus-Node.apk"

# Quick repack (no aapt2): when only Kotlin changed, compile + d8, then copy every entry of the
# previous APK except META-INF/ and classes.dex, add the new classes.dex, zipalign -p 4, apksigner.
