// Gazelle Remote: the Android shell for Gazelle's own web UI. One module, `app`.
//
// There is no Gradle wrapper in the tree: CI sets up a pinned Gradle (.github/workflows/ci.yml),
// and a developer with Gradle installed runs `gradle :app:assembleDebug` here. `gradle wrapper`
// adds one whenever that is wanted (README.md).

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
    }
}

rootProject.name = "gazelle-remote"
include(":app")
