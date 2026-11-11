# Signing and notarizing the macOS app

The release workflow signs Ond.app or Rede.app and its .dmg with a Developer ID certificate and sends the .dmg to Apple's notary service. A signed, notarized app opens on the first try, without the Open Anyway step. The work happens in `scripts/bundle-macos.sh` and `scripts/bundle-rede-macos.sh`. Both apps use the same five repository secrets. With no secrets set, the app is signed ad hoc.

You need a paid Apple Developer Program membership. Only the account holder can create a Developer ID certificate.

## 1. The Developer ID Application certificate

1. Open Keychain Access. Choose Keychain Access › Certificate Assistant › Request a Certificate From a Certificate Authority. Enter your email, leave CA Email empty, choose Saved to disk, and save the `.certSigningRequest` file.
2. Go to [developer.apple.com/account/resources/certificates](https://developer.apple.com/account/resources/certificates/list), click +, choose Developer ID Application, and pick the G2 Sub-CA. Upload the request and download the `.cer` file.
3. Double-click the `.cer` file to add it to your login keychain. Check that it worked:

   ```sh
   security find-identity -v -p codesigning
   ```

   It should list `Developer ID Application: Your Name (TEAMID)`.
4. In Keychain Access, under My Certificates, right-click the certificate (it has the private key under it) and choose Export. Save it as `.p12` and set a password.

Keep the `.p12` and its password somewhere safe, such as a password manager. Anyone with both can sign apps as you. The certificate lasts five years. Apps signed and notarized before it expires keep opening after it expires.

## 2. A notary API key

1. Go to [App Store Connect › Users and Access › Integrations › Team Keys](https://appstoreconnect.apple.com/access/integrations/api) and create a key with the Developer role.
2. Download the `.p8` file. Apple lets you download it only once.
3. Note the Key ID (in the key's row) and the Issuer ID (above the list).

## 3. Repository secrets

In GitHub, go to Settings › Secrets and variables › Actions and add:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE` | The `.p12`, base64: `base64 -i Ond.p12 \| pbcopy` |
| `MACOS_CERTIFICATE_PASSWORD` | The `.p12` password |
| `APPLE_NOTARY_KEY` | The whole `.p8` file, including the BEGIN and END lines: `pbcopy < AuthKey_XXXX.p8` |
| `APPLE_NOTARY_KEY_ID` | The Key ID |
| `APPLE_NOTARY_ISSUER` | The Issuer ID |

With only the first two set, the app is signed but not notarized. macOS still blocks the first launch in that case, so set all five.

## Trying it locally

With the certificate in your keychain and the `.p8` on disk:

```sh
OND_SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
OND_NOTARY_KEY=~/keys/AuthKey_XXXX.p8 OND_NOTARY_KEY_ID=XXXX OND_NOTARY_ISSUER=xxxxxxxx-... \
  scripts/bundle-macos.sh
```

For Rede, use `scripts/bundle-rede-macos.sh` with the same variables. Rede also accepts `REDE_SIGN_IDENTITY` and `REDE_NOTARY_KEY` / `REDE_NOTARY_KEY_ID` / `REDE_NOTARY_ISSUER` overrides. Its `--dev` mode always uses an ad-hoc signature and skips notarization.

Notarization usually takes a few minutes. To check the result (replace Ond with Rede for that app):

```sh
spctl --assess --type execute --verbose=2 target/bundle/Ond.app   # "source=Notarized Developer ID"
xcrun stapler validate target/bundle/Ond-*.dmg
```

If Apple rejects a submission, `xcrun notarytool log <submission id> --key … --key-id … --issuer …` says why.
