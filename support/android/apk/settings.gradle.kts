import java.util.Properties

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

include(":servoapp")

val localPropertiesFile = File("local.properties")
if (localPropertiesFile.exists()) {
    println("Loading local.properties")
    val props = Properties()
    localPropertiesFile.inputStream().use {
        props.load(it)
    }
    props.forEach { (key, value) ->
        println("$key = $value")
        gradle.extra.set(key!!.toString(), value)
    }
    if (props.containsKey("servoViewLocal")) {
        println("Using local build of servoview")
        include (":servoview-local")
        project(":servoview-local").projectDir = File("servoview-local")
    } else {
        include(":servoview")
    }
} else {
    include(":servoview")
}