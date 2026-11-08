// Application Android d'AuroDownloader (SPEC §4.4, phase 4).
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // Partie Kotlin de rustls-platform-verifier (vérification des certificats par
        // Android), publiée par le projet rustls à côté de la crate.
        maven {
            url = uri("https://github.com/rustls/rustls-platform-verifier/raw/maven-archive/android-release-support/maven/")
        }
    }
}

rootProject.name = "AuroDownloader"
include(":app")
