import java.io.File

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Racine du dépôt (espace de travail Cargo) : deux dossiers au-dessus de apps/android.
val repoRoot: File = rootProject.projectDir.parentFile.parentFile

android {
    namespace = "org.aurodownloader"
    compileSdk = 35
    // Le NDK qui compile le moteur (CLAUDE.md, compilation Android).
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "org.aurodownloader"
        // Android 8.0 et plus (N-08).
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        // Tests instrumentés (accessibilité, étape 7c) : androidTest/.
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            // Le moteur n'est compilé que pour arm64 pour l'instant.
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs"))
}

kotlin {
    jvmToolchain(17)
}

// ---------- Moteur : bibliothèque native compilée par Cargo ----------

/// Construit `libauro_android.so` (crate `auro-android`) pour arm64, API 26, avec le NDK
/// d'Android, et le range là où l'application prend ses bibliothèques natives.
val cargoBuild by tasks.registering(Exec::class) {
    val ndk = android.ndkDirectory
    val host = if (System.getProperty("os.name").startsWith("Windows")) "windows-x86_64" else "linux-x86_64"
    val bin = File(ndk, "toolchains/llvm/prebuilt/$host/bin")
    val ext = if (host.startsWith("windows")) ".cmd" else ""
    val exe = if (host.startsWith("windows")) ".exe" else ""
    val clang = File(bin, "aarch64-linux-android26-clang$ext").path
    workingDir = repoRoot
    environment("CC_aarch64_linux_android", clang)
    environment("AR_aarch64_linux_android", File(bin, "llvm-ar$exe").path)
    environment("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", clang)
    commandLine("cargo", "build", "-p", "auro-android", "--target", "aarch64-linux-android", "--release")
    val out = layout.buildDirectory.dir("rustJniLibs/arm64-v8a")
    doLast {
        val so = File(repoRoot, "target/aarch64-linux-android/release/libauro_android.so")
        val dir = out.get().asFile
        dir.mkdirs()
        so.copyTo(File(dir, so.name), overwrite = true)
    }
}

tasks.named("preBuild") { dependsOn(cargoBuild) }

// ---------- Partie Kotlin de rustls-platform-verifier ----------

/// Version de rustls-platform-verifier-android dans Cargo.lock : la partie Kotlin doit être
/// exactement celle de la crate (README de rustls-platform-verifier).
abstract class RustlsVersion : ValueSource<String, RustlsVersion.Params> {
    interface Params : ValueSourceParameters {
        val lockFile: RegularFileProperty
    }

    companion object {
        const val CRATE_NAME = "rustls-platform-verifier-android"
    }

    override fun obtain(): String {
        val lines = parameters.lockFile.get().asFile.readLines()
        val at = lines.indexOfFirst { it.trim() == "name = \"$CRATE_NAME\"" }
        val version = if (at < 0) {
            null
        } else {
            lines.drop(at + 1)
                .firstOrNull { it.trimStart().startsWith("version = ") }
                ?.substringAfter('"', "")
                ?.substringBefore('"', "")
                ?.takeIf { it.isNotEmpty() }
        }
        return version ?: error("$CRATE_NAME absent de Cargo.lock")
    }
}

val rustlsPlatformVerifierVersion = providers.of(RustlsVersion::class.java) {
    parameters.lockFile.set(File(repoRoot, "Cargo.lock"))
}

configurations.configureEach {
    resolutionStrategy.eachDependency {
        if (requested.group == "org.rustls" && requested.name == "rustls-platform-verifier") {
            useVersion(rustlsPlatformVerifierVersion.get())
            because("la partie Kotlin doit être celle de la crate")
        }
    }
}

dependencies {
    implementation("org.rustls:rustls-platform-verifier:latest.release")

    // Interface (étape 3) : Jetpack Compose et Material 3 (SPEC §4.3).
    implementation(platform("androidx.compose:compose-bom:2025.05.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.core:core-ktx:1.16.0")
    debugImplementation("androidx.compose.ui:ui-tooling")

    // Accessibilité (étape 7c, SPEC 9.1) : tests Compose sur l'appareil, contrôles de
    // l'Accessibility Test Framework de Google (enableAccessibilityChecks).
    androidTestImplementation(platform("androidx.compose:compose-bom:2025.05.00"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4-accessibility")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test:rules:1.6.1")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
