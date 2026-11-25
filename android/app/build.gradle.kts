plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "app.opencleaner"
    compileSdk = 36
    defaultConfig {
        applicationId = "app.opencleaner"
        minSdk = 30 // MediaStore.createTrashRequest needs API 30
        targetSdk = 36
        versionCode = 1
        versionName = rootProject.file("../VERSION").readText().trim()
    }
    buildTypes {
        release { signingConfig = signingConfigs.getByName("debug"); isMinifyEnabled = true; proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt")) }
    }
    buildFeatures { compose = true }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2025.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.12.4")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.4")
    implementation("androidx.core:core-ktx:1.17.0")
    // On-device only; models ship inside the app, nothing is uploaded.
    implementation("com.google.mlkit:face-detection:16.1.7")
    implementation("com.google.mlkit:image-labeling:17.0.9")
    implementation("com.google.mlkit:text-recognition:16.0.1")
}
