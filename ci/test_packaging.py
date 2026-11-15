import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

import fetch_ffmpeg


class MediaDownloads(unittest.TestCase):
    def test_rejects_archive_without_checksum(self):
        with patch.object(fetch_ffmpeg, "download", return_value=b"0" * 64 + b"  other.zip\n"):
            with self.assertRaisesRegex(RuntimeError, "Missing checksum"):
                fetch_ffmpeg.verified_download("https://example.invalid", "test.zip", "checksums")

    def test_rejects_wrong_checksum(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(fetch_ffmpeg, "DOWNLOADS", Path(temp)):
            with patch.object(fetch_ffmpeg, "download", side_effect=[b"0" * 64 + b"  test.zip\n", b"corrupted"]):
                with self.assertRaisesRegex(RuntimeError, "Checksum mismatch"):
                    fetch_ffmpeg.verified_download("https://example.invalid", "test.zip", "checksums")

    def test_revalidates_cached_download(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(fetch_ffmpeg, "DOWNLOADS", Path(temp)):
            (Path(temp) / "test.zip").write_bytes(b"stale")
            expected = hashlib.sha256(b"valid").hexdigest().encode() + b"  test.zip\n"
            with patch.object(fetch_ffmpeg, "download", side_effect=[expected, b"valid"]):
                result = fetch_ffmpeg.verified_download("https://example.invalid", "test.zip", "checksums")
                self.assertEqual(result.read_bytes(), b"valid")

    def test_only_extracts_helpers_and_notices(self):
        with tempfile.TemporaryDirectory() as temp:
            archive = Path(temp) / "test.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("build/bin/ffmpeg.exe", b"ffmpeg")
                bundle.writestr("build/bin/ffprobe.exe", b"ffprobe")
                bundle.writestr("build/bin/avcodec.dll", b"dll")
                bundle.writestr("build/LICENSE.txt", b"license")
                bundle.writestr("../../outside.txt", b"should not escape")
                bundle.writestr("build/bin/ffplay.exe", b"unused")
            output = Path(temp) / "media"
            with patch.object(fetch_ffmpeg, "verified_download", return_value=archive):
                fetch_ffmpeg.fetch("windows", output)
            self.assertEqual({p.name for p in output.iterdir()},
                             {"ffmpeg.exe", "ffprobe.exe", "avcodec.dll", "LICENSE.txt", "SOURCE.txt"})
            self.assertFalse((Path(temp) / "outside.txt").exists())


if __name__ == "__main__":
    unittest.main()
