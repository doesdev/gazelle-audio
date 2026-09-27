package io.github.doesdev.gazelle.remote

import android.content.Context
import android.content.Intent
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.ProgressBar
import com.github.takahirom.roborazzi.RoborazziOptions
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.time.Duration
import java.util.concurrent.CountDownLatch
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Every screen of the app's own, rendered on the JVM and compared with the committed images in
 * src/test/screenshots: `gradlew :app:verifyRoborazziDebug` checks them, and
 * `gradlew :app:recordRoborazziDebug` records them again after a change on purpose.
 *
 * Each test puts the real activity into a state through its real code: the pairing screen as a
 * first run opens it, the connecting and trouble screens by what the computer answers. Only the
 * two things a JVM has no answer for are stood in: the network (the activity's `probe`) and Play
 * services (`scannerAvailable`). The downloadable fonts do not resolve here, so the images show
 * the fallback sans serif, the same on every run.
 *
 * Sizes: a typical phone (411 by 891dp), a small one (360 by 640dp) for the pairing screen, and a
 * large font (1.3) for the pairing and trouble screens. All at mdpi, so one image pixel is one dp
 * and the files stay small.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
// Android 15 (API 35): Robolectric runs API 36 only on Java 21, and the build's JDK is 17.
@Config(sdk = [35], qualifiers = PHONE)
class ScreensTest {

    private val options = RoborazziOptions(
        // Text is antialiased by the platform's own renderer; a different machine can move a few
        // edge pixels, and nothing more than that may pass.
        compareOptions = RoborazziOptions.CompareOptions(changeThreshold = 0.002F),
    )

    private val paired = "http://192.168.1.20:8420"
    private val remoteOff = """{"error":{"code":"remote_off","message":"phones are not allowed"}}"""

    /** Start the activity, as a first run or with this phone already paired with [paired]. */
    private fun launch(
        savedBase: String? = null,
        intent: Intent? = null,
        canScan: Boolean = true,
        probe: (String) -> ProbeResult = { ProbeResult.Answered(200, "") },
    ): MainActivity {
        val context: Context = RuntimeEnvironment.getApplication()
        context.getSharedPreferences("gazelle", Context.MODE_PRIVATE).edit().apply {
            if (savedBase == null) remove("base_url") else putString("base_url", savedBase)
        }.commit()
        val controller = if (intent == null) {
            Robolectric.buildActivity(MainActivity::class.java)
        } else {
            Robolectric.buildActivity(MainActivity::class.java, intent)
        }
        controller.get().probe = probe
        controller.get().scannerAvailable = { canScan }
        return controller.setup().get()
    }

    /** Run the main thread's queue until `done`, while the probe's thread answers. */
    private fun settle(activity: MainActivity, done: (MainActivity) -> Boolean) {
        repeat(200) {
            shadowOf(Looper.getMainLooper()).idle()
            if (done(activity)) return
            Thread.sleep(10)
        }
        throw AssertionError("the screen never settled")
    }

    private fun MainActivity.shows(id: Int) = findViewById<View>(id).visibility == View.VISIBLE

    private fun MainActivity.capture(name: String) {
        shadowOf(Looper.getMainLooper()).idle()
        assertThumbSized(window.decorView)
        window.decorView.captureRoboImage("src/test/screenshots/$name.png", options)
    }

    /** Every button and field on screen is at least 48dp tall, and none is wider than the screen. */
    private fun assertThumbSized(view: View) {
        if (view.visibility != View.VISIBLE) return
        if (view is Button || view is EditText) {
            val density = view.resources.displayMetrics.density
            assertTrue("${view.javaClass.simpleName} is ${view.height / density}dp tall", view.height >= 48 * density)
            assertTrue("${view.javaClass.simpleName} is clipped", view.width <= view.rootView.width)
        }
        if (view is ViewGroup) for (i in 0 until view.childCount) assertThumbSized(view.getChildAt(i))
    }

    // The pairing screen.

    @Test
    fun pairing() {
        launch().capture("pairing")
    }

    @Test
    @Config(qualifiers = SMALL)
    fun pairingSmall() {
        launch().capture("pairing_w360")
    }

    @Test
    fun pairingLargeFont() {
        RuntimeEnvironment.setFontScale(1.3f)
        launch().capture("pairing_font130")
    }

    @Test
    fun pairingWithoutPlayServices() {
        launch(canScan = false).capture("pairing_no_play_services")
    }

    @Test
    fun pairingTyped() {
        val activity = launch()
        activity.findViewById<EditText>(R.id.pair_address).setText("192.168.1.20:8420")
        activity.findViewById<EditText>(R.id.pair_code).setText("YHV8-YRJM")
        activity.capture("pairing_typed")
    }

    @Test
    fun pairingRefused() {
        val activity = launch()
        activity.findViewById<EditText>(R.id.pair_address).setText("192.168.1.20:8420")
        activity.findViewById<EditText>(R.id.pair_code).setText("YHV8")
        activity.findViewById<View>(R.id.pair_submit).performClick()
        activity.capture("pairing_refused")
    }

    @Test
    fun pairingWhilePaired() {
        val intent = Intent("io.github.doesdev.gazelle.remote.PAIR")
            .setClassName("io.github.doesdev.gazelle.remote", MainActivity::class.java.name)
        val activity = launch(savedBase = paired, intent = intent)
        assertTrue(activity.shows(R.id.pair_back))
        activity.capture("pairing_while_paired")
    }

    // Connecting, and each way it can end on the trouble screen.

    /** A probe that answers only when the test says so, so "Connecting" can be caught as it is. */
    private class HeldProbe {
        val release = CountDownLatch(1)
        val probe: (String) -> ProbeResult = {
            release.await()
            ProbeResult.NoResponse(NoAnswer.TIMED_OUT, "")
        }
    }

    @Test
    fun connecting() {
        val held = HeldProbe()
        try {
            val activity = launch(savedBase = paired, probe = held.probe)
            assertTrue(activity.shows(R.id.connecting_screen))
            // The spinner is there, in the accent; a JVM draws its first frame, which is empty.
            val spinner = activity.findViewById<ProgressBar>(R.id.connecting_progress)
            assertTrue(spinner.visibility == View.VISIBLE && spinner.isIndeterminate)
            assertEquals(activity.getColor(R.color.accent), spinner.indeterminateTintList?.defaultColor)
            activity.capture("connecting")
        } finally {
            held.release.countDown()
        }
    }

    @Test
    fun troubleCancelled() {
        val held = HeldProbe()
        try {
            val activity = launch(savedBase = paired, probe = held.probe)
            activity.findViewById<View>(R.id.connecting_cancel).performClick()
            assertTrue(activity.shows(R.id.trouble_screen))
            activity.capture("trouble_cancelled")
        } finally {
            held.release.countDown()
        }
    }

    private fun trouble(probe: (String) -> ProbeResult): MainActivity {
        val activity = launch(savedBase = paired, probe = probe)
        settle(activity) { it.shows(R.id.trouble_screen) }
        return activity
    }

    @Test
    fun troubleTimedOut() {
        trouble { ProbeResult.NoResponse(NoAnswer.TIMED_OUT, "connect timed out") }.capture("trouble_timed_out")
    }

    @Test
    fun troubleTimedOutLargeFont() {
        RuntimeEnvironment.setFontScale(1.3f)
        trouble { ProbeResult.NoResponse(NoAnswer.TIMED_OUT, "connect timed out") }.capture("trouble_timed_out_font130")
    }

    @Test
    fun troubleRefused() {
        trouble { ProbeResult.NoResponse(NoAnswer.REFUSED, "Connection refused") }.capture("trouble_refused")
    }

    @Test
    fun troublePhonesOff() {
        trouble { ProbeResult.Answered(403, remoteOff) }.capture("trouble_phones_off")
    }

    @Test
    fun troublePageTooSlow() {
        // The computer answers; the page (which a JVM WebView never loads) does not show within
        // the watchdog's 15 seconds.
        val asked = AtomicBoolean(false)
        val activity = launch(savedBase = paired, probe = { asked.set(true); ProbeResult.Answered(200, "") })
        settle(activity) { asked.get() }
        // The answer is posted as the probe returns; let it arrive, which asks for the page.
        Thread.sleep(100)
        shadowOf(Looper.getMainLooper()).idle()
        assertTrue(activity.shows(R.id.connecting_screen))
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(Reach.WATCHDOG_MS + 1_000))
        assertTrue(activity.shows(R.id.trouble_screen))
        activity.capture("trouble_page_too_slow")
    }
}

/** A typical phone, and a small one; mdpi, so the images are small. */
private const val PHONE = "w411dp-h891dp-port-mdpi"
private const val SMALL = "w360dp-h640dp-port-mdpi"
