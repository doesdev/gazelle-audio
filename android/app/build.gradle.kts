plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.roborazzi)
}

// The release workflow passes the server's version, so the app and the Gazelle it pairs with carry
// the same number: -PgazelleVersionName=1.5.0 -PgazelleVersionCode=10500, the code being
// major * 10000 + minor * 100 + patch. A build without them is a developer's, and says so.
val gazelleVersionName: String = providers.gradleProperty("gazelleVersionName").getOrElse("0.0.0-dev")
val gazelleVersionCode: Int = providers.gradleProperty("gazelleVersionCode").getOrElse("1").toInt()

android {
    // The applicationId is the app's identity for ever: an installed copy only accepts an update
    // with the same one, signed by the same key. It is the reverse of doesdev.github.io, the
    // GitHub Pages domain of the account that publishes Gazelle, so it is one nobody else can hold.
    namespace = "io.github.doesdev.gazelle.remote"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.doesdev.gazelle.remote"
        minSdk = 26
        targetSdk = 36
        versionCode = gazelleVersionCode
        versionName = gazelleVersionName
    }

    buildTypes {
        release {
            // Unsigned here: the release workflow signs it in its protected environment.
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        // For BuildConfig.VERSION_NAME, which the app adds to its user agent.
        buildConfig = true
    }

    lint {
        abortOnError = true
    }

    testOptions {
        // Robolectric renders the app's own layouts and styles in the screenshot tests.
        unitTests.isIncludeAndroidResources = true
    }
}

// The screenshot tests' images: recorded with recordRoborazziDebug, checked with
// verifyRoborazziDebug (CI), committed beside the tests.
roborazzi {
    outputDir.set(file("src/test/screenshots"))
}

dependencies {
    implementation(libs.androidx.activity)
    implementation(libs.androidx.core.ktx)
    implementation(libs.play.services.code.scanner)
    testImplementation(libs.junit)
    testImplementation(libs.robolectric)
    testImplementation(libs.roborazzi)
}
