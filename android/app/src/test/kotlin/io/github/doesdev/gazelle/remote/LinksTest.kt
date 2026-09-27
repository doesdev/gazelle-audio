package io.github.doesdev.gazelle.remote

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LinksTest {

    private fun ok(checked: Checked<PairLink>): PairLink = when (checked) {
        is Checked.Ok -> checked.value
        is Checked.Refused -> throw AssertionError("refused: ${checked.reason}")
    }

    private fun refusal(checked: Checked<PairLink>): String = when (checked) {
        is Checked.Ok -> throw AssertionError("accepted: ${checked.value}")
        is Checked.Refused -> checked.reason
    }

    private val home = Origin("192.168.1.20", 8420)

    @Test
    fun `a scanned code as Gazelle writes it is accepted, for IPv4, IPv6 and a name`() {
        val link = ok(Links.parseScanned("http://192.168.1.20:8420/pair#code=YHV8-YRJM"))
        assertEquals(home, link.origin)
        assertEquals("YHV8-YRJM", link.code)
        assertEquals("http://192.168.1.20:8420/pair#code=YHV8-YRJM", link.url)
        assertEquals("http://192.168.1.20:8420/#/remote", link.origin.remote)

        assertEquals(Origin("[fe80::1]", 8420), ok(Links.parseScanned("http://[fe80::1]:8420/pair#code=0000-ZZZZ")).origin)
        assertEquals(Origin("[2001:db8:0:0:0:0:0:1]", 9000), ok(Links.parseScanned("http://[2001:db8:0:0:0:0:0:1]:9000/pair#code=0000-ZZZZ")).origin)
        assertEquals(Origin("studio-pc", 8420), ok(Links.parseScanned("http://Studio-PC:8420/pair#code=ABCD-EFGH")).origin)
        assertEquals(Origin("studio-pc.local", 8420), ok(Links.parseScanned("http://studio-pc.local:8420/pair#code=ABCD-EFGH")).origin)
        // Surrounding white space from a scanner is not the code's.
        assertEquals(home, ok(Links.parseScanned("  http://192.168.1.20:8420/pair#code=YHV8-YRJM\n")).origin)
    }

    @Test
    fun `anything else scanned is refused with the same plain message`() {
        for (text in listOf(
            "",
            "hello",
            "https://192.168.1.20:8420/pair#code=YHV8-YRJM", // https
            "ftp://192.168.1.20:8420/pair#code=YHV8-YRJM",
            "http://192.168.1.20/pair#code=YHV8-YRJM", // no port
            "http://192.168.1.20:0/pair#code=YHV8-YRJM",
            "http://192.168.1.20:65536/pair#code=YHV8-YRJM",
            "http://192.168.1.20:08420/pair#code=YHV8-YRJM",
            "http://192.168.1.20:/pair#code=YHV8-YRJM",
            "http://192.168.1.20:8420/pair/#code=YHV8-YRJM", // path not exactly /pair
            "http://192.168.1.20:8420/Pair#code=YHV8-YRJM",
            "http://192.168.1.20:8420/other/pair#code=YHV8-YRJM",
            "http://192.168.1.20:8420/pair?x=1#code=YHV8-YRJM", // a query
            "http://192.168.1.20:8420/pair#code=YHV8-YRJM&x=1", // more in the fragment
            "http://192.168.1.20:8420/pair#code=yhv8-yrjm", // not as Gazelle writes it
            "http://192.168.1.20:8420/pair#code=YHV8YRJM",
            "http://192.168.1.20:8420/pair#code=YHV8-YRJU", // U is not in the alphabet
            "http://192.168.1.20:8420/pair#code=",
            "http://192.168.1.20:8420/pair",
            "http://192.168.1.20:8420/pair#other=YHV8-YRJM",
            "http://user@192.168.1.20:8420/pair#code=YHV8-YRJM", // a user name
            "http://evil.example@192.168.1.20:8420/pair#code=YHV8-YRJM",
            "http://192.168.1.20:8420@evil.example/pair#code=YHV8-YRJM",
            "http://192.168.1.256:8420/pair#code=YHV8-YRJM",
            "http://192.168.01.20:8420/pair#code=YHV8-YRJM", // a leading zero
            "http://1.2.3:8420/pair#code=YHV8-YRJM", // looks like IPv4, is not
            "http://[fe80::1%25eth0]:8420/pair#code=YHV8-YRJM", // a zone
            "http://[fe80::1:8420/pair#code=YHV8-YRJM",
            "http://fe80::1:8420/pair#code=YHV8-YRJM", // IPv6 without brackets
            "http://[1:2:3:4:5:6:7:8:9]:8420/pair#code=YHV8-YRJM",
            "http://-bad.name:8420/pair#code=YHV8-YRJM",
            "http://bad_name:8420/pair#code=YHV8-YRJM",
            "http://bücher:8420/pair#code=YHV8-YRJM", // not ASCII
            "http:/192.168.1.20:8420/pair#code=YHV8-YRJM",
            "http://:8420/pair#code=YHV8-YRJM",
            "http:///pair#code=YHV8-YRJM",
            "javascript:alert(1)//http://192.168.1.20:8420/pair#code=YHV8-YRJM",
        )) {
            assertEquals(text, Links.NOT_A_PAIRING_CODE, refusal(Links.parseScanned(text)))
        }
    }

    @Test
    fun `a typed address may leave out http, the port and the path, and the code is read as Gazelle reads it`() {
        assertEquals(PairLink(home, "YHV8-YRJM"), ok(Links.parseTyped("192.168.1.20:8420", "YHV8-YRJM")))
        assertEquals(PairLink(home, "YHV8-YRJM"), ok(Links.parseTyped(" 192.168.1.20 ", "yhv8 yrjm")))
        assertEquals(PairLink(home, "YHV8-YRJM"), ok(Links.parseTyped("http://192.168.1.20:8420/", "YHV8YRJM")))
        assertEquals(PairLink(home, "YHV8-YRJM"), ok(Links.parseTyped("HTTP://192.168.1.20:8420/pair", "y-h-v-8-y-r-j-m")))
        assertEquals(Origin("studio-pc", 9000), ok(Links.parseTyped("studio-pc:9000", "YHV8-YRJM")).origin)
        // O, I and L are read as 0, 1 and 1, as the computer reads them.
        assertEquals("0111-0000", ok(Links.parseTyped("192.168.1.20", "OIL1-0000")).code)
        // A whole pairing address pasted in is read as a scan.
        assertEquals(PairLink(home, "YHV8-YRJM"), ok(Links.parseTyped("http://192.168.1.20:8420/pair#code=YHV8-YRJM", "")))
    }

    @Test
    fun `a typed address or code that cannot be one says which`() {
        assertEquals(Links.NO_ADDRESS, refusal(Links.parseTyped("  ", "YHV8-YRJM")))
        assertEquals(Links.HTTPS_ADDRESS, refusal(Links.parseTyped("https://192.168.1.20:8420", "YHV8-YRJM")))
        for (address in listOf("192.168.1.20:99999", "user@192.168.1.20", "192.168.1.20/mixer", "a b", "192.168.1.20:8420?x")) {
            assertEquals(address, Links.BAD_ADDRESS, refusal(Links.parseTyped(address, "YHV8-YRJM")))
        }
        for (code in listOf("", "YHV8-YRJ", "YHV8-YRJMM", "YHV8-YRJU", "YHV8_YRJM")) {
            assertEquals(code, Links.BAD_CODE, refusal(Links.parseTyped("192.168.1.20:8420", code)))
        }
    }

    @Test
    fun `the same origin means the same scheme, host and port`() {
        assertTrue(Links.sameOrigin("http://192.168.1.20:8420/", home))
        assertTrue(Links.sameOrigin("http://192.168.1.20:8420/#/remote", home))
        assertTrue(Links.sameOrigin("http://192.168.1.20:8420/api/v1/health?x=1", home))
        assertTrue(Links.sameOrigin("http://192.168.1.20:8420", home))
        assertTrue(Links.sameOrigin("HTTP://STUDIO-PC:8420/", Origin("studio-pc", 8420)))

        assertFalse("another scheme", Links.sameOrigin("https://192.168.1.20:8420/", home))
        assertFalse("another host", Links.sameOrigin("http://192.168.1.21:8420/", home))
        assertFalse("another port", Links.sameOrigin("http://192.168.1.20:8421/", home))
        assertFalse("the default port is 80", Links.sameOrigin("http://192.168.1.20/", home))
        assertFalse(Links.sameOrigin("http://192.168.1.20:8420.evil.example/", home))
        assertFalse(Links.sameOrigin("http://192.168.1.20:8420@evil.example/", home))
        assertFalse(Links.sameOrigin("http://evil.example/?http://192.168.1.20:8420/", home))
        assertFalse(Links.sameOrigin("https://github.com/doesdev/gazelle-audio", home))
        assertFalse(Links.sameOrigin("about:blank", home))
        assertFalse(Links.sameOrigin("", home))

        assertTrue(Links.sameOrigin("http://example.org/", Origin("example.org", 80)))
        assertEquals(Origin("[::1]", 8420), Links.originOf("http://[::1]:8420/x"))
        assertNull(Links.originOf("mailto:someone@example.org"))
    }

    @Test
    fun `the pair page without a code is the native pairing screen's, and with one it is the web page's`() {
        assertTrue(Links.isBarePairPage("http://192.168.1.20:8420/pair", home))
        assertTrue(Links.isBarePairPage("http://192.168.1.20:8420/pair/", home))
        assertTrue(Links.isBarePairPage("http://192.168.1.20:8420/pair#", home))
        assertTrue(Links.isBarePairPage("http://192.168.1.20:8420/pair#code=", home))
        assertTrue(Links.isBarePairPage("http://192.168.1.20:8420/pair?x=1", home))

        assertFalse(Links.isBarePairPage("http://192.168.1.20:8420/pair#code=YHV8-YRJM", home))
        assertFalse(Links.isBarePairPage("http://192.168.1.20:8420/", home))
        assertFalse(Links.isBarePairPage("http://192.168.1.20:8420/#/pair", home))
        assertFalse("another computer's", Links.isBarePairPage("http://192.168.1.21:8420/pair", home))
    }

    @Test
    fun `the remote page is the one the pair page lands on`() {
        assertTrue(Links.isRemotePage("http://192.168.1.20:8420/#/remote", home))
        assertTrue(Links.isRemotePage("http://192.168.1.20:8420#/remote", home))
        assertTrue(Links.isRemotePage("http://192.168.1.20:8420/#/remote?device=1", home))

        assertFalse(Links.isRemotePage("http://192.168.1.20:8420/#/mixer", home))
        assertFalse(Links.isRemotePage("http://192.168.1.20:8420/#/remoteish", home))
        assertFalse(Links.isRemotePage("http://192.168.1.20:8420/remote", home))
        assertFalse(Links.isRemotePage("http://192.168.1.20:8420/pair", home))
        assertFalse("another computer's", Links.isRemotePage("http://192.168.1.21:8420/#/remote", home))
    }

    @Test
    fun `only web and mail links are handed to other apps`() {
        assertTrue(Links.opensOutside("https://github.com/doesdev/gazelle-audio"))
        assertTrue(Links.opensOutside("http://example.org/"))
        assertTrue(Links.opensOutside("mailto:someone@example.org"))
        assertFalse(Links.opensOutside("intent://scan/#Intent;scheme=zxing;end"))
        assertFalse(Links.opensOutside("file:///sdcard/x"))
        assertFalse(Links.opensOutside("content://x/y"))
        assertFalse(Links.opensOutside("javascript:alert(1)"))
    }

    @Test
    fun `host checks`() {
        assertTrue(Links.isIpv4("0.0.0.0"))
        assertTrue(Links.isIpv4("255.255.255.255"))
        assertFalse(Links.isIpv4("256.0.0.1"))
        assertFalse(Links.isIpv4("1.2.3"))
        assertFalse(Links.isIpv4("1.2.3.4.5"))
        assertFalse(Links.isIpv4("01.2.3.4"))

        assertTrue(Links.isIpv6("::"))
        assertTrue(Links.isIpv6("::1"))
        assertTrue(Links.isIpv6("fe80::1"))
        assertTrue(Links.isIpv6("::ffff:192.168.1.20"))
        assertTrue(Links.isIpv6("1:2:3:4:5:6:7:8"))
        assertFalse(Links.isIpv6("1:2:3:4:5:6:7"))
        assertFalse(Links.isIpv6("1::2::3"))
        assertFalse(Links.isIpv6(":1:2:3:4:5:6:7"))
        assertFalse(Links.isIpv6("12345::1"))
        assertFalse(Links.isIpv6("fe80::1%eth0"))

        assertTrue(Links.isHostName("studio-pc"))
        assertTrue(Links.isHostName("studio-pc.local"))
        assertFalse(Links.isHostName("studio-"))
        assertFalse(Links.isHostName("a..b"))
        assertFalse(Links.isHostName("1.2.3"))
        assertFalse(Links.isHostName(""))
    }

    @Test
    fun `a code is normalised as the computer does it`() {
        assertEquals("YHV8-YRJM", Links.normaliseCode("yhv8yrjm"))
        assertEquals("YHV8-YRJM", Links.normaliseCode(" YHV8 - YRJM "))
        assertNull(Links.normaliseCode("YHV8-YRJ"))
        assertNull(Links.normaliseCode("YHV8-YRJMX"))
        assertNull(Links.normaliseCode("YHV8-YRJÉ"))
    }

}
