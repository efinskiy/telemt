"""Offline regression tests for atomic bounded database installation."""
import gzip
import io
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch
import update_geoip


class Response(io.BytesIO):
    status = 200

    def geturl(self):
        return "https://download.db-ip.com/free/database.mmdb.gz"


class GeoIpInstallTests(unittest.TestCase):
    def test_bounded_copy_rejects_oversize_and_deadline(self):
        with self.assertRaises(ValueError):
            update_geoip.bounded_copy(io.BytesIO(b"12345"), io.BytesIO(), 4, time.monotonic() + 10)
        with self.assertRaises(TimeoutError):
            update_geoip.bounded_copy(io.BytesIO(), io.BytesIO(), 4, time.monotonic() - 1)

    def test_failed_download_and_invalid_database_preserve_previous_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "city.mmdb"
            path.write_bytes(b"old database")
            for payload in [b"not gzip", gzip.compress(b"not mmdb")]:
                with patch("urllib.request.urlopen", return_value=Response(payload)):
                    with self.assertRaises((OSError, ValueError)):
                        update_geoip.install(path, "2026-10")
                self.assertEqual(path.read_bytes(), b"old database")
                self.assertEqual(list(Path(directory).glob(".geoip-*")), [])

    def test_valid_container_is_replaced_with_readable_permissions(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "city.mmdb"
            payload = b"test metadata envelope" + update_geoip.METADATA_MARKER
            with patch("urllib.request.urlopen", return_value=Response(gzip.compress(payload))):
                update_geoip.install(path, "2026-10")
            self.assertEqual(path.read_bytes(), payload)
            self.assertEqual(path.stat().st_mode & 0o777, 0o644)


if __name__ == "__main__":
    unittest.main()
