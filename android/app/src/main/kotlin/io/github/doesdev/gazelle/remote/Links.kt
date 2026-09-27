package io.github.doesdev.gazelle.remote

// Addresses, and which ones the app will open. Plain Kotlin with no Android types, so the unit
// tests under src/test run it on the JVM, and so every decision about where the WebView may go is
// made here, in one place, under test.
//
// The parsing is written out by hand rather than handed to a URL library, because the point is to
// refuse: a scanned code is accepted only in exactly the shape Gazelle puts in its QR code, and
// "the same origin" means the same scheme, host and port, compared as text.

/** A Gazelle's address: plain http, a host and a port. The host is lower case, IPv6 in brackets. */
data class Origin(val host: String, val port: Int) {
    /** `http://host:port`, what the app remembers. */
    val base: String get() = "http://$host:$port"

    /** The page a paired phone opens on. */
    val remote: String get() = "$base/#/remote"
}

/** A pairing address: the computer, and the code it is showing. */
data class PairLink(val origin: Origin, val code: String) {
    /** What the WebView opens: Gazelle's own pair page, which does the pairing. */
    val url: String get() = "${origin.base}/pair#code=$code"
}

/** What was asked for, or why not, in words for the person holding the phone. */
sealed class Checked<out T> {
    data class Ok<out T>(val value: T) : Checked<T>()
    data class Refused(val reason: String) : Checked<Nothing>()
}

object Links {
    /** Gazelle's port unless it was started with another. */
    const val DEFAULT_PORT = 8420

    /** Crockford's base 32: the alphabet Gazelle draws its pairing codes from (no I, L, O or U). */
    private const val CODE_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    private val CODE_SHAPE = Regex("^[$CODE_ALPHABET]{4}-[$CODE_ALPHABET]{4}$")
    private const val PAIR_TAIL = "/pair#code="

    const val NOT_A_PAIRING_CODE =
        "That is not a Gazelle pairing code. Scan the QR code Gazelle shows on the computer when you choose Pair a phone."
    const val NO_ADDRESS = "Type the computer's address, as Gazelle shows it under Phones, such as 192.168.1.20:8420."
    const val BAD_ADDRESS =
        "That is not an address Gazelle would show. Type it as the Phones section has it, such as 192.168.1.20:8420."
    const val HTTPS_ADDRESS =
        "Gazelle on your network uses plain http, not https. Type the address as Gazelle shows it, such as 192.168.1.20:8420."
    const val BAD_CODE = "The code is 8 letters and digits, such as YHV8-YRJM, as the computer shows it."

    /**
     * A scanned QR code, accepted only as Gazelle writes it: `http://`, an IP address (IPv4, or
     * IPv6 in brackets) or a plain host name, an explicit port, the path `/pair`, and a fragment
     * that is `code=` and a code of the shape `XXXX-XXXX`. Nothing more and nothing less.
     */
    fun parseScanned(text: String): Checked<PairLink> {
        val refused = Checked.Refused(NOT_A_PAIRING_CODE)
        val raw = text.trim()
        if (!raw.startsWith("http://", ignoreCase = true)) return refused
        val rest = raw.substring("http://".length)
        val slash = rest.indexOf('/')
        if (slash <= 0) return refused
        val tail = rest.substring(slash)
        if (!tail.startsWith(PAIR_TAIL)) return refused
        val code = tail.substring(PAIR_TAIL.length)
        if (!CODE_SHAPE.matches(code)) return refused
        val origin = parseAuthority(rest.substring(0, slash), defaultPort = null) ?: return refused
        return Checked.Ok(PairLink(origin, code))
    }

    /**
     * What a person typed: the address as the computer shows it (`192.168.1.20:8420`, with or
     * without `http://` in front or `/` or `/pair` after, and the port 8420 when left out), and the
     * code, read as Gazelle reads it: case, spaces and dashes do not matter, and O, I and L are
     * taken for 0, 1 and 1. A whole pairing address pasted into the address box is read as a scan.
     */
    fun parseTyped(address: String, code: String): Checked<PairLink> {
        var text = address.trim()
        if (text.contains("#code=")) return parseScanned(text)
        if (text.startsWith("https://", ignoreCase = true)) return Checked.Refused(HTTPS_ADDRESS)
        if (text.startsWith("http://", ignoreCase = true)) text = text.substring("http://".length)
        text = text.removeSuffix("/")
        if (text.endsWith("/pair", ignoreCase = true)) text = text.dropLast("/pair".length)
        if (text.isEmpty()) return Checked.Refused(NO_ADDRESS)
        val origin = parseAuthority(text, defaultPort = DEFAULT_PORT) ?: return Checked.Refused(BAD_ADDRESS)
        val normal = normaliseCode(code) ?: return Checked.Refused(BAD_CODE)
        return Checked.Ok(PairLink(origin, normal))
    }

    /** A code as a person typed it, in the form Gazelle shows it, or null if it cannot be one. */
    fun normaliseCode(input: String): String? {
        val chars = StringBuilder()
        for (c in input) {
            val upper = when (val u = c.uppercaseChar()) {
                ' ', '-' -> continue
                'O' -> '0'
                'I', 'L' -> '1'
                else -> u
            }
            if (upper !in CODE_ALPHABET || chars.length == 8) return null
            chars.append(upper)
        }
        return if (chars.length == 8) "${chars.substring(0, 4)}-${chars.substring(4)}" else null
    }

    /** The origin of an `http://` URL, the port 80 when it has none; null for anything else. */
    fun originOf(url: String): Origin? {
        if (!url.startsWith("http://", ignoreCase = true)) return null
        val rest = url.substring("http://".length)
        return parseAuthority(rest.substring(0, authorityEnd(rest)), defaultPort = 80)
    }

    /** Whether `url` is on `origin`: the same scheme (http), host and port. */
    fun sameOrigin(url: String, origin: Origin): Boolean = originOf(url) == origin

    /**
     * Gazelle's pair page with no code, on `origin`: where the "Pair again" link of a phone that is
     * no longer paired goes. The app shows its own pairing screen instead, since the page alone
     * could only ask for a code that has to come from the computer anyway.
     */
    fun isBarePairPage(url: String, origin: Origin): Boolean {
        if (!sameOrigin(url, origin)) return false
        val parts = parts(url)
        if (parts.path != "/pair" && parts.path != "/pair/") return false
        val fragment = parts.fragment ?: return true
        return fragment.split('&').none { it.startsWith("code=") && it.length > "code=".length }
    }

    /** Gazelle's Remote page on `origin`, which is where the pair page lands once it has paired. */
    fun isRemotePage(url: String, origin: Origin): Boolean {
        if (!sameOrigin(url, origin)) return false
        val parts = parts(url)
        val fragment = parts.fragment ?: return false
        return (parts.path == "" || parts.path == "/") &&
            (fragment == "/remote" || fragment.startsWith("/remote/") || fragment.startsWith("/remote?"))
    }

    /** Whether a link off the paired origin may be handed to another app: web and mail only. */
    fun opensOutside(url: String): Boolean =
        url.startsWith("http://", ignoreCase = true) ||
            url.startsWith("https://", ignoreCase = true) ||
            url.startsWith("mailto:", ignoreCase = true)

    /** Whether `url` is on the web at all, and so something the WebView could fetch. */
    fun isWeb(url: String): Boolean =
        url.startsWith("http://", ignoreCase = true) || url.startsWith("https://", ignoreCase = true)

    private class Parts(val path: String, val fragment: String?)

    /** The path and fragment of an `http://` URL (the query dropped). */
    private fun parts(url: String): Parts {
        val hash = url.indexOf('#')
        val fragment = if (hash < 0) null else url.substring(hash + 1)
        val beforeHash = if (hash < 0) url else url.substring(0, hash)
        val rest = beforeHash.substringAfter("://", "")
        val pathAndQuery = rest.substring(authorityEnd(rest))
        return Parts(pathAndQuery.substringBefore('?'), fragment)
    }

    /** Where the authority of `rest` (a URL after its `scheme://`) ends. */
    private fun authorityEnd(rest: String): Int {
        val end = rest.indexOfFirst { it == '/' || it == '?' || it == '#' }
        return if (end < 0) rest.length else end
    }

    /**
     * `host:port`, or `host` alone when there is a `defaultPort`. The host is an IPv4 address in
     * plain dotted decimal, an IPv6 address in brackets (no zone), or a host name of letters,
     * digits, dots and hyphens. Anything else, a user name or a stray character included, is null.
     */
    internal fun parseAuthority(authority: String, defaultPort: Int?): Origin? {
        val host: String
        val portText: String?
        if (authority.startsWith("[")) {
            val close = authority.indexOf(']')
            if (close < 0) return null
            host = authority.substring(0, close + 1)
            val after = authority.substring(close + 1)
            portText = when {
                after.isEmpty() -> null
                after.startsWith(":") -> after.substring(1)
                else -> return null
            }
            if (!isIpv6(host.substring(1, host.length - 1))) return null
        } else {
            val colon = authority.indexOf(':')
            host = if (colon < 0) authority else authority.substring(0, colon)
            portText = if (colon < 0) null else authority.substring(colon + 1)
            if (!isIpv4(host) && !isHostName(host)) return null
        }
        val port = if (portText == null) defaultPort ?: return null else parsePort(portText) ?: return null
        return Origin(host.lowercase(), port)
    }

    private fun isAsciiDigit(c: Char) = c in '0'..'9'

    private fun isHexDigit(c: Char) = c in '0'..'9' || c in 'a'..'f' || c in 'A'..'F'

    private fun parsePort(text: String): Int? {
        if (text.isEmpty() || text.length > 5 || !text.all { isAsciiDigit(it) } || text.startsWith('0')) return null
        return text.toInt().takeIf { it in 1..65535 }
    }

    /** Four decimal numbers up to 255, with no leading zeros (which some parsers read as octal). */
    internal fun isIpv4(text: String): Boolean {
        val parts = text.split('.')
        return parts.size == 4 && parts.all { part ->
            part.length in 1..3 && part.all { isAsciiDigit(it) } && (part == "0" || !part.startsWith('0')) && part.toInt() <= 255
        }
    }

    /** An IPv6 address without its brackets: eight groups, or fewer with one `::`, and no zone. */
    internal fun isIpv6(text: String): Boolean {
        if (text.isEmpty() || text.length > 45) return false
        val halves = text.split("::")
        if (halves.size > 2) return false
        val groups = halves.flatMap { if (it.isEmpty()) emptyList() else it.split(':') }
        var count = 0
        for ((i, group) in groups.withIndex()) {
            when {
                group.isEmpty() -> return false
                i == groups.lastIndex && halves.last().endsWith(group) && group.contains('.') -> {
                    if (!isIpv4(group)) return false
                    count += 2
                }
                group.length in 1..4 && group.all { isHexDigit(it) } -> count += 1
                else -> return false
            }
        }
        return if (halves.size == 2) count <= 7 else count == 8
    }

    /**
     * A host name: dot separated labels of ASCII letters, digits and hyphens, none starting or
     * ending with a hyphen. A name whose last label is all digits is refused, since a browser
     * would read it as a malformed IPv4 address rather than as a name.
     */
    internal fun isHostName(text: String): Boolean {
        if (text.isEmpty() || text.length > 253) return false
        val labels = text.split('.')
        val valid = labels.all { label ->
            label.length in 1..63 && !label.startsWith('-') && !label.endsWith('-') &&
                label.all { it in 'a'..'z' || it in 'A'..'Z' || isAsciiDigit(it) || it == '-' }
        }
        return valid && !labels.last().all { isAsciiDigit(it) }
    }
}
