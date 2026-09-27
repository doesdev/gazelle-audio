package io.github.doesdev.gazelle.remote

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.CookieManager
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebStorage
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.core.net.toUri
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.isVisible
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import java.io.ByteArrayInputStream

/**
 * The whole app: Gazelle's own web page, full screen, confined to the one computer this phone is
 * paired with, plus a native screen to pair (scan the QR code, or type the address and code) and
 * one for when the page does not load.
 *
 * Pairing is the web page's: the app opens `http://host:port/pair#code=...`, the page exchanges
 * the code for the phone's key, which the server sets as an HttpOnly cookie in the WebView's
 * cookie jar, and lands on `/#/remote`. The app remembers `http://host:port` once the WebView has
 * reached that page, and opens it there from then on.
 */
class MainActivity : ComponentActivity() {

    private lateinit var web: WebView
    private lateinit var pairScreen: View
    private lateinit var troubleScreen: View
    private lateinit var pairCurrent: TextView
    private lateinit var pairBack: Button
    private lateinit var pairScan: Button
    private lateinit var pairScanUnavailable: TextView
    private lateinit var pairAddress: EditText
    private lateinit var pairCode: EditText
    private lateinit var pairMessage: TextView
    private lateinit var troubleTitle: TextView
    private lateinit var troubleDetail: TextView

    private val prefs by lazy { getSharedPreferences(PREFS, Context.MODE_PRIVATE) }

    /** The computer this phone is paired with: set once pairing has reached its Remote page. */
    private var paired: Origin? = null

    /** The pairing under way, until its Remote page opens. */
    private var pairing: PairLink? = null

    /**
     * The one origin the WebView may load anything from: the pairing under way, or else the paired
     * computer. Read on the WebView's own threads too, hence volatile.
     */
    @Volatile
    private var confinedTo: Origin? = null

    /** Clear the WebView's history once the next page on the confined origin has loaded. */
    private var clearHistoryOnLoad = false

    /** The WebView's renderer has gone, so it must not be touched again. */
    private var webGone = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContentView(R.layout.activity_main)

        val root = findViewById<View>(R.id.root)
        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            val bars = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout() or
                    WindowInsetsCompat.Type.ime(),
            )
            view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            WindowInsetsCompat.CONSUMED
        }

        web = findViewById(R.id.web)
        pairScreen = findViewById(R.id.pair_screen)
        troubleScreen = findViewById(R.id.trouble_screen)
        pairCurrent = findViewById(R.id.pair_current)
        pairBack = findViewById(R.id.pair_back)
        pairScan = findViewById(R.id.pair_scan)
        pairScanUnavailable = findViewById(R.id.pair_scan_unavailable)
        pairAddress = findViewById(R.id.pair_address)
        pairCode = findViewById(R.id.pair_code)
        pairMessage = findViewById(R.id.pair_message)
        troubleTitle = findViewById(R.id.trouble_title)
        troubleDetail = findViewById(R.id.trouble_detail)

        pairBack.setOnClickListener { openRemote() }
        pairScan.setOnClickListener { scan() }
        findViewById<Button>(R.id.pair_submit).setOnClickListener { typed() }
        findViewById<Button>(R.id.trouble_retry).setOnClickListener { retry() }
        findViewById<Button>(R.id.trouble_repair).setOnClickListener { forget { showPair() } }

        setUpWebView()
        onBackPressedDispatcher.addCallback(this, back)

        paired = prefs.getString(KEY_BASE, null)?.let { Links.originOf(it) }
        if (intent?.action == ACTION_PAIR || paired == null) showPair() else openRemote()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (intent.action == ACTION_PAIR) showPair()
    }

    override fun onResume() {
        super.onResume()
        if (!webGone) web.onResume()
    }

    override fun onPause() {
        // The key is a cookie: write the jar out whenever the app leaves the screen.
        CookieManager.getInstance().flush()
        if (!webGone) web.onPause()
        super.onPause()
    }

    override fun onDestroy() {
        if (!webGone) {
            webGone = true
            web.destroy()
        }
        super.onDestroy()
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun setUpWebView() {
        web.setBackgroundColor(ContextCompat.getColor(this, R.color.background))
        web.settings.apply {
            // Gazelle's page is a JavaScript app that keeps its preferences in local storage.
            javaScriptEnabled = true
            domStorageEnabled = true
            // Nothing on the phone is the page's business.
            allowFileAccess = false
            allowContentAccess = false
            // Gazelle is plain http, and a page on it has no reason to load anything over https
            // either; anything off the origin is refused below regardless.
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            javaScriptCanOpenWindowsAutomatically = false
            setSupportMultipleWindows(false)
            setGeolocationEnabled(false)
            // So Gazelle can tell this app from a browser: "GazelleRemote/1.5.0" at the end.
            userAgentString = "$userAgentString GazelleRemote/${BuildConfig.VERSION_NAME}"
        }
        // Safe Browsing is left at its default (on). No JavaScript interface is ever added.
        CookieManager.getInstance().setAcceptCookie(true)
        web.webViewClient = client
    }

    private val client = object : WebViewClient() {

        override fun shouldOverrideUrlLoading(view: WebView?, request: WebResourceRequest?): Boolean {
            if (request == null) return true
            val url = request.url.toString()
            val origin = confinedTo
            if (origin != null && Links.isBarePairPage(url, origin)) {
                // "Pair again" on a phone that is no longer paired: the native pairing screen.
                showPair()
                return true
            }
            if (origin != null && Links.sameOrigin(url, origin)) return false
            // Anywhere else (GitHub, the manual) is the system browser's, and only when a person
            // tapped a link, so a page cannot throw anyone out of the app on its own.
            if (request.isForMainFrame && request.hasGesture()) openOutside(url)
            return true
        }

        override fun shouldInterceptRequest(view: WebView?, request: WebResourceRequest?): WebResourceResponse? {
            if (request == null) return null
            val url = request.url.toString()
            if (!Links.isWeb(url)) return null
            val origin = confinedTo
            if (origin != null && Links.sameOrigin(url, origin)) return null
            // Whatever the page asks for from another origin is refused: the WebView talks to the
            // paired computer and nothing else.
            return WebResourceResponse("text/plain", "utf-8", 403, "Forbidden", emptyMap(), ByteArrayInputStream(ByteArray(0)))
        }

        // onPageFinished does not fire when only the fragment changes, and the pair page reaches
        // `/#/remote` by navigating, which this sees either way.
        override fun doUpdateVisitedHistory(view: WebView?, url: String?, isReload: Boolean) {
            val link = pairing ?: return
            if (url != null && Links.isRemotePage(url, link.origin)) pairedWith(link.origin)
        }

        override fun onPageFinished(view: WebView?, url: String?) {
            val origin = confinedTo ?: return
            if (clearHistoryOnLoad && url != null && Links.sameOrigin(url, origin)) {
                // Back from the Remote page must not lead to the pair page, or to a blank one.
                clearHistoryOnLoad = false
                web.clearHistory()
            }
        }

        override fun onReceivedError(view: WebView?, request: WebResourceRequest?, error: WebResourceError?) {
            if (request == null || !request.isForMainFrame) return
            val where = describe(request.url.toString())
            showTrouble(getString(R.string.trouble_not_answering, where), error?.description?.toString().orEmpty())
        }

        override fun onReceivedHttpError(view: WebView?, request: WebResourceRequest?, errorResponse: WebResourceResponse?) {
            if (request == null || errorResponse == null || !request.isForMainFrame) return
            val status = errorResponse.statusCode
            val where = describe(request.url.toString())
            if (status == 403) {
                showTrouble(getString(R.string.trouble_refused, where), getString(R.string.trouble_refused_detail))
            } else {
                showTrouble(getString(R.string.trouble_http, where), getString(R.string.trouble_http_detail, status))
            }
        }

        override fun onRenderProcessGone(view: WebView?, detail: RenderProcessGoneDetail?): Boolean {
            // The page's renderer crashed or was stopped for memory. This WebView can never be
            // used again, so it goes, and the activity starts over with a new one.
            if (!webGone) {
                webGone = true
                (web.parent as? ViewGroup)?.removeView(web)
                web.destroy()
            }
            recreate()
            return true
        }
    }

    private val back = object : OnBackPressedCallback(true) {
        override fun handleOnBackPressed() {
            when {
                // Leaving the pair page before it has paired: back to the native pairing screen.
                web.isVisible && pairing != null -> showPair()
                web.isVisible && web.canGoBack() -> web.goBack()
                pairScreen.isVisible && paired != null -> openRemote()
                else -> {
                    isEnabled = false
                    onBackPressedDispatcher.onBackPressed()
                    isEnabled = true
                }
            }
        }
    }

    /** Show one of the three screens and hide the others. */
    private fun show(screen: View) {
        for (each in listOf(web, pairScreen, troubleScreen)) each.isVisible = each === screen
    }

    private fun load(url: String) {
        if (!webGone) web.loadUrl(url)
    }

    /** The paired computer's Remote page, full screen. */
    private fun openRemote() {
        val origin = paired ?: return showPair()
        pairing = null
        confinedTo = origin
        show(web)
        load(origin.remote)
    }

    /** The pairing screen: scan, or type. Says which computer this phone is paired with, if any. */
    private fun showPair(message: String = "") {
        if (pairing != null) {
            pairing = null
            confinedTo = paired
        }
        val current = paired
        pairCurrent.isVisible = current != null
        pairBack.isVisible = current != null
        if (current != null) pairCurrent.text = getString(R.string.pair_current, current.base)
        val canScan = GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(this) == ConnectionResult.SUCCESS
        pairScan.isVisible = canScan
        pairScanUnavailable.isVisible = !canScan
        pairMessage.text = message
        show(pairScreen)
    }

    private fun scan() {
        pairMessage.text = ""
        val options = GmsBarcodeScannerOptions.Builder()
            .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
            .build()
        GmsBarcodeScanning.getClient(this, options)
            .startScan()
            .addOnSuccessListener { barcode -> scanned(barcode.rawValue.orEmpty()) }
            .addOnFailureListener { pairMessage.text = getString(R.string.pair_scan_failed) }
    }

    private fun scanned(text: String) {
        when (val checked = Links.parseScanned(text)) {
            is Checked.Ok -> startPairing(checked.value)
            is Checked.Refused -> pairMessage.text = checked.reason
        }
    }

    private fun typed() {
        when (val checked = Links.parseTyped(pairAddress.text.toString(), pairCode.text.toString())) {
            is Checked.Ok -> startPairing(checked.value)
            is Checked.Refused -> pairMessage.text = checked.reason
        }
    }

    /**
     * Pair with `link`'s computer: forget any earlier pairing, then open Gazelle's pair page there.
     * The page asks for a name for the phone and pairs; [doUpdateVisitedHistory] sees it land.
     */
    private fun startPairing(link: PairLink) {
        forget {
            pairing = link
            confinedTo = link.origin
            clearHistoryOnLoad = true
            show(web)
            load(link.url)
        }
    }

    /** The pair page has reached the Remote page: this phone is paired with `origin` now. */
    private fun pairedWith(origin: Origin) {
        paired = origin
        pairing = null
        confinedTo = origin
        prefs.edit { putString(KEY_BASE, origin.base) }
        CookieManager.getInstance().flush()
        clearHistoryOnLoad = true
    }

    /** Retry whatever did not load: the pairing under way, or the paired computer. */
    private fun retry() {
        val link = pairing
        if (link != null) {
            show(web)
            load(link.url)
        } else {
            openRemote()
        }
    }

    /**
     * Forget the paired computer: the saved address, the phone's key (every cookie) and the page's
     * own storage. Then `then`, once the cookies are gone.
     */
    private fun forget(then: () -> Unit) {
        paired = null
        pairing = null
        confinedTo = null
        prefs.edit { remove(KEY_BASE) }
        load("about:blank")
        WebStorage.getInstance().deleteAllData()
        val cookies = CookieManager.getInstance()
        cookies.removeAllCookies { _ ->
            cookies.flush()
            then()
        }
    }

    private fun showTrouble(title: String, detail: String) {
        troubleTitle.text = title
        troubleDetail.text = detail
        troubleDetail.isVisible = detail.isNotEmpty()
        show(troubleScreen)
    }

    /** `http://host:port` for a URL on a Gazelle, or the URL itself. */
    private fun describe(url: String): String = Links.originOf(url)?.base ?: url

    /** Hand a link off the paired computer to the phone's browser (or mail app). */
    private fun openOutside(url: String) {
        if (!Links.opensOutside(url)) return
        val intent = Intent(Intent.ACTION_VIEW, url.toUri()).addCategory(Intent.CATEGORY_BROWSABLE)
        try {
            startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            // Nothing on the phone opens it; the link stays untaken.
        }
    }

    private companion object {
        const val PREFS = "gazelle"
        const val KEY_BASE = "base_url"
        const val ACTION_PAIR = "io.github.doesdev.gazelle.remote.PAIR"
    }
}
