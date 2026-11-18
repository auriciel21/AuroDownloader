import java.io.File

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Racine du dépôt (espace de travail Cargo) : deux dossiers au-dessus de apps/android.
val repoRoot: File = rootProject.projectDir.parentFile.parentFile

/// Architectures du moteur : arm64 par défaut (appareils) ; `-PauroAbis=x86_64` ou
/// `-PauroAbis=arm64-v8a,x86_64` pour l'émulateur de l'intégration continue (étape 8).
val rustTargets = mapOf("arm64-v8a" to "aarch64-linux-android", "x86_64" to "x86_64-linux-android")
val auroAbis: List<String> = (findProperty("auroAbis") as String? ?: "arm64-v8a")
    .split(",").map { it.trim() }.filter { it.isNotEmpty() }
    .onEach { require(it in rustTargets) { "auroAbis : $it inconnue (${rustTargets.keys})" } }

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
            // Les architectures du moteur (`auroAbis`).
            abiFilters += auroAbis
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

/// Construit `libauro_android.so` (crate `auro-android`) pour une architecture, API 26, avec
/// le NDK d'Android, et le range là où l'application prend ses bibliothèques natives.
val cargoBuilds = auroAbis.map { abi ->
    val target = rustTargets.getValue(abi)
    val env = target.replace('-', '_')
    tasks.register<Exec>("cargoBuild-$abi") {
        val ndk = android.ndkDirectory
        val host = if (System.getProperty("os.name").startsWith("Windows")) "windows-x86_64" else "linux-x86_64"
        val bin = File(ndk, "toolchains/llvm/prebuilt/$host/bin")
        val ext = if (host.startsWith("windows")) ".cmd" else ""
        val exe = if (host.startsWith("windows")) ".exe" else ""
        val clang = File(bin, "${target}26-clang$ext").path
        workingDir = repoRoot
        environment("CC_$env", clang)
        environment("AR_$env", File(bin, "llvm-ar$exe").path)
        environment("CARGO_TARGET_${env.uppercase()}_LINKER", clang)
        commandLine("cargo", "build", "-p", "auro-android", "--target", target, "--release")
        val out = layout.buildDirectory.dir("rustJniLibs/$abi")
        doLast {
            val so = File(repoRoot, "target/$target/release/libauro_android.so")
            val dir = out.get().asFile
            dir.mkdirs()
            so.copyTo(File(dir, so.name), overwrite = true)
        }
    }
}

tasks.named("preBuild") { dependsOn(cargoBuilds) }

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
