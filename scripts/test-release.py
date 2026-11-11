#!/usr/bin/env python3
"""Release plumbing without a compiler, signing identity, server or desktop."""

import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

# Only external build/Apple tools are faked. The bundle scripts, plist, tar,
# desktop entry, icons and install/uninstall operations are the shipped ones.
MOCK = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
name = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["TOOL_LOG"], "a") as log:
    log.write(json.dumps([name, *args]) + "\n")
if name == "cargo":
    package = args[args.index("-p") + 1]
    if args[0] == "pkgid":
        version = os.environ.get("TEST_VERSION", "0.5.0" if package == "ond-player" else "0.1.0-beta.1")
        fragment = f"{package}@{version}" if package == "ond-player" else version
        print(f"path+file:///fixture/apps/{package}#{fragment}")
    elif os.environ.get("FAIL_BUILD"):
        sys.exit(1)
    else:
        target = Path("target")
        if "--target" in args:
            target /= args[args.index("--target") + 1]
        target /= "release"
        target.mkdir(parents=True, exist_ok=True)
        (target / ("Ond" if package == "ond-player" else "Rede")).write_text("fixture binary\n")
elif name == "uname":
    print("x86_64")
elif name == "git":
    print("123")
elif name == "lipo":
    Path(args[args.index("-output") + 1]).write_text("universal fixture\n")
elif name == "hdiutil":
    if os.environ.get("FAIL_DMG"):
        sys.exit(1)
    Path(args[-1]).touch()
elif name == "ditto":
    Path(args[-1]).touch()
elif name == "xcrun" and os.environ.get("FAIL_NOTARY"):
    sys.exit(1)
'''


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for directory in ["scripts", "packaging", "docs/rede"]:
            shutil.copytree(ROOT / directory, self.root / directory)
        for name in ["README.md", "LICENSE-MIT", "LICENSE-APACHE"]:
            shutil.copy2(ROOT / name, self.root / name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        for name in ["cargo", "uname", "git", "lipo", "hdiutil", "ditto", "codesign", "plutil", "xcrun", "spctl", "sleep", "update-desktop-database", "gtk-update-icon-cache"]:
            path = self.bin / name
            path.write_text(MOCK)
            path.chmod(0o755)
        # macOS's BSD install lacks -D; use an installed GNU-compatible tool.
        if sys.platform == "darwin":
            install = shutil.which("ginstall") or shutil.which("uu-install")
            if install:
                (self.bin / "install").symlink_to(install)
        self.env = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("OND_", "REDE_", "GITHUB_", "TEST_", "FAIL_"))
        }
        self.env.update(PATH=f"{self.bin}:{os.environ['PATH']}", TOOL_LOG=str(self.root / "tools.jsonl"))

    def run_script(self, script, *args, success=True, **env):
        interpreter = "sh" if Path(script).name == "install.sh" else "bash"
        result = subprocess.run(
            [interpreter, str(self.root / script), *args], cwd=self.root,
            env={**self.env, **env}, text=True, capture_output=True,
        )
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def calls(self):
        return [json.loads(line) for line in (self.root / "tools.jsonl").read_text().splitlines()]

    def test_each_tag_selects_only_its_own_app_and_exact_version(self):
        for tag, app, version, pre in [
            ("ond-v0.5.0", "ond", "0.5.0", "false"),
            ("v0.5.0", "ond", "0.5.0", "false"),
            ("rede-v0.1.0-beta.1", "rede", "0.1.0-beta.1", "true"),
        ]:
            with self.subTest(tag=tag):
                out = self.run_script("scripts/release-tag.sh", GITHUB_REF=f"refs/tags/{tag}").stdout
                values = dict(line.split("=", 1) for line in out.splitlines())
                self.assertEqual(values, dict(app=app, name=app.capitalize(), version=version, prerelease=pre))
        for tag in ["ond-v0.4.4", "rede-v0.1.0", "rede-v0.1.0-beta.2", "ond-v", "rede-v", "v", "unrelated"]:
            with self.subTest(tag=tag):
                self.run_script("scripts/release-tag.sh", success=False, GITHUB_REF=f"refs/tags/{tag}")

    def test_manual_builds_select_either_app_without_a_tag(self):
        for app in ["ond", "rede"]:
            out = self.run_script("scripts/release-tag.sh", app, GITHUB_REF="refs/heads/dev").stdout
            self.assertIn(f"app={app}\n", out)
        self.run_script("scripts/release-tag.sh", "other", success=False)

    def test_linux_archives_install_and_uninstall_without_touching_the_other_app(self):
        if sys.platform == "darwin" and not (self.bin / "install").exists():
            self.skipTest("needs GNU install (or uu-install) on macOS")
        prefix = self.root / "prefix with spaces"
        for app, version in [("ond", "0.5.0"), ("rede", "0.1.0-beta.1")]:
            script = "scripts/bundle-linux.sh" if app == "ond" else "scripts/bundle-rede-linux.sh"
            self.run_script(script)
            name = f"{app.capitalize()}-{version}-linux-x86_64"
            archive = self.root / "target/bundle" / f"{name}.tar.gz"
            unpack = self.root / "unpacked" / app
            unpack.mkdir(parents=True)
            with tarfile.open(archive) as bundle:
                bundle.extractall(unpack, filter="data")
            folder = unpack / name
            self.assertTrue((folder / "README.md").is_file())
            self.assertTrue((folder / "LICENSE-MIT").is_file())
            self.run_script(str(folder / "install.sh"), str(prefix))
            self.assertTrue(os.access(prefix / "bin" / app, os.X_OK))
            entry = prefix / f"share/applications/dev.solidsilver.{app}.desktop"
            self.assertIn(f'Exec="{prefix}/bin/{app}"\n', entry.read_text())
            self.assertTrue((prefix / f"share/icons/hicolor/256x256/apps/dev.solidsilver.{app}.png").is_file())
        self.assertIn("Rede beta", (folder / "README.md").read_text())
        self.run_script(str(folder / "install.sh"), "--uninstall", str(prefix))
        self.assertFalse((prefix / "bin/rede").exists())
        self.assertFalse(entry.exists())
        self.assertTrue((prefix / "bin/ond").is_file())
        self.assertTrue((prefix / "share/applications/dev.solidsilver.ond.desktop").is_file())

    def test_macos_beta_is_notarized_before_zipping_and_has_a_numeric_plist_version(self):
        self.run_script(
            "scripts/bundle-rede-macos.sh", "--universal",
            OND_SIGN_IDENTITY="fixture identity", OND_NOTARY_KEY="fixture.p8",
            OND_NOTARY_KEY_ID="fixture-id", OND_NOTARY_ISSUER="fixture-issuer",
        )
        out = self.root / "target/bundle"
        self.assertTrue((out / "Rede-0.1.0-beta.1-macos-universal.dmg").is_file())
        self.assertTrue((out / "Rede-0.1.0-beta.1-macos-universal.zip").is_file())
        with (out / "Rede.app/Contents/Info.plist").open("rb") as file:
            plist = plistlib.load(file)
        self.assertEqual(plist["CFBundleShortVersionString"], "0.1.0")
        self.assertEqual(plist["CFBundleVersion"], "123")
        self.assertEqual(plist["CFBundleIdentifier"], "dev.solidsilver.rede")
        schemes = plist["CFBundleURLTypes"][0]["CFBundleURLSchemes"]
        self.assertEqual(schemes, ["rede", "audiobookshelf"])
        calls = self.calls()
        stapled_app = calls.index(["xcrun", "stapler", "staple", "target/bundle/Rede.app"])
        zipped = next(i for i, call in enumerate(calls) if call[0] == "ditto")
        self.assertLess(stapled_app, zipped)
        self.assertEqual(sum(call[:2] == ["cargo", "build"] for call in calls), 2)

    def test_dev_bundles_ignore_signing_credentials_and_never_notarize(self):
        self.run_script("scripts/bundle-rede-macos.sh", "--dev", REDE_SIGN_IDENTITY="fixture identity", REDE_NOTARY_KEY="fixture.p8")
        calls = self.calls()
        self.assertIn(["codesign", "--force", "--sign", "-", "target/bundle/Rede.app"], calls)
        self.assertFalse(any(call[0] in ["hdiutil", "xcrun", "ditto"] for call in calls))

    def test_failed_builds_and_notarization_never_produce_an_archive(self):
        self.run_script("scripts/bundle-rede-linux.sh", success=False, FAIL_BUILD="1")
        self.assertFalse(list(self.root.glob("target/bundle/*.tar.gz")))
        self.run_script(
            "scripts/bundle-rede-macos.sh", success=False, REDE_SIGN_IDENTITY="fixture identity",
            REDE_NOTARY_KEY="fixture.p8", REDE_NOTARY_KEY_ID="fixture-id",
            REDE_NOTARY_ISSUER="fixture-issuer", FAIL_NOTARY="1",
        )
        self.assertFalse(any(call[0] == "ditto" for call in self.calls()))

    def test_incomplete_notarization_credentials_are_rejected(self):
        self.run_script("scripts/bundle-rede-macos.sh", success=False, REDE_SIGN_IDENTITY="fixture identity", REDE_NOTARY_KEY="fixture.p8")
        self.assertFalse(any(call[0] in ["hdiutil", "ditto"] for call in self.calls()))


if __name__ == "__main__":
    unittest.main()
