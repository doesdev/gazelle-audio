package io.github.doesdev.gazelle.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.ConnectException
import java.net.InetAddress
import java.net.NoRouteToHostException
import java.net.ServerSocket
import java.net.SocketException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import kotlin.concurrent.thread

class ReachTest {

    private val remoteOff = """{"error":{"code":"remote_off","message":"phones are not allowed"}}"""

    @Test
    fun `any HTTP answer means load the page, except Gazelle saying phones are off`() {
        assertEquals(AfterProbe.Load, Reach.next(ProbeResult.Answered(200, "<!doctype html>")))
        assertEquals(AfterProbe.PhonesOff, Reach.next(ProbeResult.Answered(403, remoteOff)))
        // Another 403 (a name that is not the computer's, say) is still Gazelle answering.
        assertEquals(AfterProbe.Load, Reach.next(ProbeResult.Answered(403, """{"error":{"code":"bad_host"}}""")))
        assertEquals(AfterProbe.Load, Reach.next(ProbeResult.Answered(403, "")))
        for (status in listOf(301, 401, 404, 500, 503)) {
            assertEquals("$status", AfterProbe.Load, Reach.next(ProbeResult.Answered(status, remoteOff)))
        }
        // HttpURLConnection says -1 for an answer that is not HTTP.
        val notHttp = Reach.next(ProbeResult.Answered(-1, ""))
        assertTrue("$notHttp", notHttp is AfterProbe.NotAnswering && notHttp.why == NoAnswer.OTHER)
    }

    @Test
    fun `no answer goes to the trouble screen, saying why`() {
        for (why in NoAnswer.entries) {
            assertEquals(AfterProbe.NotAnswering(why, "x"), Reach.next(ProbeResult.NoResponse(why, "x")))
        }
    }

    @Test
    fun `failures are told apart by their exceptions`() {
        assertEquals(NoAnswer.TIMED_OUT, Reach.classify(SocketTimeoutException("connect timed out")).why)
        assertEquals(NoAnswer.REFUSED, Reach.classify(ConnectException("failed to connect: ECONNREFUSED (Connection refused)")).why)
        assertEquals(NoAnswer.REFUSED, Reach.classify(ConnectException("Connection refused")).why)
        assertEquals(NoAnswer.UNREACHABLE, Reach.classify(ConnectException("failed to connect: ENETUNREACH (Network is unreachable)")).why)
        assertEquals(NoAnswer.TIMED_OUT, Reach.classify(ConnectException("failed to connect: ETIMEDOUT (Connection timed out)")).why)
        assertEquals(NoAnswer.UNREACHABLE, Reach.classify(NoRouteToHostException("No route to host")).why)
        assertEquals(NoAnswer.UNKNOWN_HOST, Reach.classify(UnknownHostException("studio-pc")).why)
        assertEquals(NoAnswer.OTHER, Reach.classify(SocketException("Connection reset")).why)
        assertEquals("studio-pc", Reach.classify(UnknownHostException("studio-pc")).detail)
    }

    /** A server on this machine that answers one request with `response`, then closes. */
    private fun answering(response: String): ServerSocket {
        val server = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        thread(isDaemon = true) {
            server.use {
                it.accept().use { socket ->
                    val reader = socket.getInputStream().bufferedReader()
                    while (reader.readLine()?.isNotEmpty() == true) Unit
                    socket.getOutputStream().write(response.toByteArray())
                    socket.getOutputStream().flush()
                }
            }
        }
        return server
    }

    private fun url(port: Int) = "http://127.0.0.1:$port/"

    @Test
    fun `a real answer is read, status and body`() {
        val body = remoteOff
        val server = answering(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: ${body.length}\r\nConnection: close\r\n\r\n$body",
        )
        val result = Reach.probe(url(server.localPort), 2_000, 2_000)
        assertEquals(ProbeResult.Answered(403, body), result)
        assertEquals(AfterProbe.PhonesOff, Reach.next(result))
    }

    @Test
    fun `a port nobody listens on is refused`() {
        val port = ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { it.localPort }
        val result = Reach.probe(url(port), 2_000, 2_000)
        assertTrue("$result", result is ProbeResult.NoResponse && result.why == NoAnswer.REFUSED)
    }

    @Test
    fun `a computer that takes the connection and never answers times out`() {
        // Listening but never accepting: the connection is made, and no answer ever comes.
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
            val started = System.nanoTime()
            val result = Reach.probe(url(server.localPort), 2_000, 300)
            assertTrue("$result", result is ProbeResult.NoResponse && result.why == NoAnswer.TIMED_OUT)
            assertTrue("gave up in time", System.nanoTime() - started < 5_000_000_000L)
        }
    }
}
