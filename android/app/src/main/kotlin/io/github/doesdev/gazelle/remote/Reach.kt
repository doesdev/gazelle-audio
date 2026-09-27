package io.github.doesdev.gazelle.remote

import java.io.IOException
import java.io.InputStream
import java.net.ConnectException
import java.net.HttpURLConnection
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.net.URI
import java.net.UnknownHostException

// Whether the computer answers at all, asked before the WebView is given the address.
//
// A Windows firewall that drops connections (a network Windows counts as Public, say) gives no
// answer at all, and a WebView left to find that out itself waits a minute or more on a white
// page. So the app first asks `http://host:port/` itself, with short timeouts, and decides from
// the answer what to show. Plain Kotlin and java.net only, with no Android types, so the unit
// tests run it on the JVM, against sockets of their own.
//
// Cleartext is allowed for this request by the same network security config that allows it for
// the WebView (res/xml/network_security_config.xml, base-config): it governs the platform's
// whole HTTP stack, HttpURLConnection included.

/** Why the computer did not answer. */
enum class NoAnswer {
    /** Nothing came back in time: a firewall dropping the connection, or a computer that is off. */
    TIMED_OUT,

    /** The computer said no one is listening on that port: Gazelle is not running, or not listening for phones. */
    REFUSED,

    /** The phone has no route to that address: another network. */
    UNREACHABLE,

    /** A host name that does not resolve. */
    UNKNOWN_HOST,

    /** Anything else that stopped the request. */
    OTHER,
}

/** What asking the computer found. */
sealed class ProbeResult {
    /** An HTTP answer, whatever its status, and the start of its body. */
    data class Answered(val status: Int, val body: String) : ProbeResult()

    /** No HTTP answer, and why. `detail` is the exception's own message, for the curious. */
    data class NoResponse(val why: NoAnswer, val detail: String) : ProbeResult()
}

/** What the app does next. */
sealed class AfterProbe {
    /** Gazelle answered: give the address to the WebView. */
    data object Load : AfterProbe()

    /** Gazelle answered that it is not letting phones in (403 `remote_off`). */
    data object PhonesOff : AfterProbe()

    /** Nothing answered: the trouble screen, saying why. */
    data class NotAnswering(val why: NoAnswer, val detail: String) : AfterProbe()
}

object Reach {
    /** How long to wait to connect, and then for the answer. */
    const val CONNECT_TIMEOUT_MS = 4_000
    const val READ_TIMEOUT_MS = 4_000

    /** How long the WebView may take, once the computer has answered, before the page shows. */
    const val WATCHDOG_MS = 15_000L

    /** How much of an answer's body is read: enough for Gazelle's error JSON. */
    private const val BODY_LIMIT = 4_096

    /**
     * The next screen for a probe's result. Any HTTP answer means the computer and Gazelle are
     * there, even a 401, 404 or 500, and the WebView takes it from there; a 403 whose body names
     * `remote_off` is Gazelle saying phones are not allowed, which the app says in its own words.
     */
    fun next(result: ProbeResult): AfterProbe = when (result) {
        is ProbeResult.Answered -> when {
            result.status == 403 && result.body.contains("remote_off") -> AfterProbe.PhonesOff
            result.status in 100..599 -> AfterProbe.Load
            else -> AfterProbe.NotAnswering(NoAnswer.OTHER, "not an HTTP answer (${result.status})")
        }
        is ProbeResult.NoResponse -> AfterProbe.NotAnswering(result.why, result.detail)
    }

    /** Why a request failed, from its exception. */
    fun classify(error: Throwable): ProbeResult.NoResponse {
        val detail = error.message.orEmpty()
        val why = when (error) {
            is SocketTimeoutException -> NoAnswer.TIMED_OUT
            is UnknownHostException -> NoAnswer.UNKNOWN_HOST
            is NoRouteToHostException -> NoAnswer.UNREACHABLE
            // Android reports ECONNREFUSED, ENETUNREACH and EHOSTUNREACH all as ConnectException.
            is ConnectException -> when {
                detail.contains("refused", ignoreCase = true) -> NoAnswer.REFUSED
                detail.contains("timed out", ignoreCase = true) || detail.contains("ETIMEDOUT") -> NoAnswer.TIMED_OUT
                else -> NoAnswer.UNREACHABLE
            }
            else -> NoAnswer.OTHER
        }
        return ProbeResult.NoResponse(why, detail)
    }

    /**
     * GET `url` with the given timeouts, following no redirect and caching nothing, and say what
     * came back. Blocks: call it off the main thread.
     */
    fun probe(url: String, connectTimeoutMs: Int = CONNECT_TIMEOUT_MS, readTimeoutMs: Int = READ_TIMEOUT_MS): ProbeResult {
        val connection = try {
            URI(url).toURL().openConnection() as HttpURLConnection
        } catch (e: IOException) {
            return classify(e)
        } catch (e: Exception) {
            // Not a URL at all (URISyntaxException, IllegalArgumentException); Links never makes one.
            return ProbeResult.NoResponse(NoAnswer.OTHER, e.message.orEmpty())
        }
        return try {
            connection.connectTimeout = connectTimeoutMs
            connection.readTimeout = readTimeoutMs
            connection.instanceFollowRedirects = false
            connection.useCaches = false
            connection.requestMethod = "GET"
            val status = connection.responseCode
            // With a status in hand the computer has answered; the body only says why a 403 is one.
            val body = try {
                val stream = if (status >= 400) connection.errorStream else connection.inputStream
                stream?.use { readUpTo(it, BODY_LIMIT) }.orEmpty()
            } catch (e: IOException) {
                ""
            }
            ProbeResult.Answered(status, body)
        } catch (e: IOException) {
            classify(e)
        } finally {
            connection.disconnect()
        }
    }

    private fun readUpTo(stream: InputStream, limit: Int): String {
        val buffer = ByteArray(limit)
        var total = 0
        while (total < limit) {
            val read = stream.read(buffer, total, limit - total)
            if (read < 0) break
            total += read
        }
        return String(buffer, 0, total, Charsets.UTF_8)
    }
}
