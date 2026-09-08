.pragma library

// One request policy for Kiosk cache requests and decode components.
function bucketFor(pixels) {
    var buckets = [50, 100, 150, 230, 300, 600]
    for (var i = 0; i < buckets.length; i++)
        if (buckets[i] >= pixels) return buckets[i]
    return 0
}

function sizedUrl(url, pixels) {
    if (!/^https?:\/\/(?:[^/]+\.)?qobuz\.com(?::[0-9]+)?\//i.test(url) || pixels <= 0) return url
    var bucket = bucketFor(pixels)
    if (!bucket) return url
    return url.replace(/_(50|100|150|230|300|600|max|org)(\.[^/?]+)(\?.*)?$/, "_" + bucket + "$2$3")
}
