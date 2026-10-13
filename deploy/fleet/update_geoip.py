#!/usr/bin/env python3
"""Install the official monthly DB-IP City Lite MMDB without exposing client IPs."""
import argparse
import datetime
import gzip
import os
from pathlib import Path
import re
import tempfile
import time
import urllib.request

MAX_COMPRESSED_BYTES = 128 * 1024 * 1024
MAX_DATABASE_BYTES = 256 * 1024 * 1024
DEADLINE_SECONDS = 300
CHUNK_BYTES = 1024 * 1024
METADATA_MARKER = b"\xab\xcd\xefMaxMind.com"


def bounded_copy(source, target, limit, deadline):
    """Enforce the decompression bound independently of any HTTP length header."""
    total = 0
    while True:
        if time.monotonic() > deadline:
            raise TimeoutError("GeoIP installation deadline exceeded")
        chunk = source.read(min(CHUNK_BYTES, limit - total + 1))
        if not chunk:
            return total
        total += len(chunk)
        if total > limit:
            raise ValueError("GeoIP database exceeds size limit")
        target.write(chunk)


def install(destination, month):
    """Replace only after download, decompression and MMDB metadata checks succeed."""
    if not re.fullmatch(r"20\d{2}-(0[1-9]|1[0-2])", month):
        raise ValueError("month must be YYYY-MM")
    url = f"https://download.db-ip.com/free/dbip-city-lite-{month}.mmdb.gz"
    destination.parent.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + DEADLINE_SECONDS
    temporary = None
    try:
        with tempfile.TemporaryFile() as compressed:
            request = urllib.request.Request(url, headers={"User-Agent": "Telemt-Fleet-GeoIP/1"})
            with urllib.request.urlopen(request, timeout=30) as response:
                if response.status != 200 or not response.geturl().startswith("https://download.db-ip.com/"):
                    raise ValueError("Unexpected GeoIP download response")
                bounded_copy(response, compressed, MAX_COMPRESSED_BYTES, deadline)
            compressed.seek(0)
            with tempfile.NamedTemporaryFile(dir=destination.parent, prefix=".geoip-", delete=False) as output:
                temporary = Path(output.name)
                with gzip.GzipFile(fileobj=compressed) as decoded:
                    size = bounded_copy(decoded, output, MAX_DATABASE_BYTES, deadline)
                output.flush()
                os.fsync(output.fileno())
            with temporary.open("rb") as check:
                check.seek(max(0, size - 131072))
                if METADATA_MARKER not in check.read(131072):
                    raise ValueError("Downloaded file has no MMDB metadata")
            os.chmod(temporary, 0o644)
            os.replace(temporary, destination)
            temporary = None
        print(f"Installed DB-IP City Lite {month}: {size} bytes. Restart the Fleet admin service to load it.")
        print("IP Geolocation by DB-IP (https://db-ip.com), CC BY 4.0.")
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--month", default=datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m"))
    parser.add_argument("--destination", type=Path, default=Path("/var/lib/telemt-geoip/dbip-city-lite.mmdb"))
    args = parser.parse_args()
    install(args.destination, args.month)
