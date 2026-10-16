# Code signing and SmartScreen

An unsigned executable downloaded from the internet makes Windows show
"Windows protected your PC" (Microsoft Defender SmartScreen) on first
run, and some antivirus heuristics flag a program that reads raw disks.
This page says what removes that warning, what does not, and how the
release workflow signs the executables once a certificate exists.

## What the warning is

SmartScreen is a reputation system, not a malware verdict. It warns when
neither the file nor its publisher has enough history of clean downloads.
Three facts decide what helps:

- **Reputation is earned, not bought.** Microsoft's own guidance states
  that Extended Validation (EV) certificates no longer skip SmartScreen:
  since the 2024 change to the Trusted Root Program requirements, every
  publisher starts with no reputation and builds it as users download and
  run signed releases. Paying for EV only to avoid the warning is not
  justified; EV still matters for kernel drivers and some corporate
  procurement.
- **Reputation attaches to the signing identity.** Without a signature,
  reputation is tracked per file hash and every release starts from zero.
  With a signature, the publisher identity accumulates reputation across
  releases, so the warning fades after a period that depends on download
  volume (days to weeks for a small project), and stays away for later
  versions signed with the same identity.
- **Verification is not reputation.** The SHA-256 sums in
  `SHA256SUMS.txt` prove a download is the file the workflow built; they
  do not tell Windows anything.

So the only honest answer to "make SmartScreen stop warning" is: sign
every release with the same certificate, keep releasing, and let the
reputation accrue. The Microsoft Store is the one shortcut (apps published
there are re-signed by Microsoft and carry full reputation), at the cost
of its packaging and review.

## Ways to get a certificate

Since 2023 the CA/Browser Forum requires code-signing keys to live in
hardware or in a cloud HSM, so a certificate is no longer a `.pfx` file
you can mail yourself. The practical options for a small open-source
project:

| option | who can use it | cost | notes |
|---|---|---|---|
| **Azure Artifact Signing** (formerly Trusted Signing) | organisations in most countries; individuals in the USA and Canada (identity validated by Microsoft) | from USD 9.99 a month | Microsoft's own service; the key never leaves Azure; a GitHub Action signs in the workflow; the certificate is short-lived and renewed automatically, reputation follows the validated identity. The best fit when eligible. |
| **OV certificate from a CA with cloud signing** (SSL.com eSigner, DigiCert KeyLocker, Certum SimplySign, GlobalSign, Sectigo) | individuals and organisations worldwide, identity validated by the CA | roughly USD 70 to 400 a year | Certum's open-source offer for individuals is the cheapest; signing happens through the CA's cloud tool or a USB token on your own machine, so the workflow step is either their CLI or a manual signing of the downloaded artefacts followed by a re-upload. |
| **SignPath Foundation** | open-source projects that apply and are accepted | free | SignPath signs the build with its own certificate through its service; the workflow submits the artefacts and polls for the signed ones. Reputation belongs to the SignPath Foundation identity. |
| **A `.pfx` you already hold** | anyone with an older exportable certificate, or a self-signed one for testing | – | The workflow supports it; a self-signed certificate does nothing for SmartScreen. |

## How the workflow signs

The *Release* workflow (`.github/workflows/release.yml`) signs the two
Windows executables (`PhoinixDR-<version>-windows-x64-portable.exe`,
`phoinix-windows-x64.exe`) right after building and checking them, with
SHA-256 digests and an RFC 3161 timestamp so the signature outlives the
certificate. It then verifies every executable with
`Get-AuthenticodeSignature` and refuses to publish a release whose
signing was configured but failed. The release notes state whether the
executables are signed and by whom. Without any of the secrets below the
release is published unsigned, and the notes say so.

### Azure Artifact Signing

1. Create an Artifact Signing account in the Azure portal, complete the
   identity validation, and create a certificate profile of type *Public
   Trust*.
2. Create an App Registration (service principal) with a client secret,
   and give it the *Trusted Signing Certificate Profile Signer* role on the
   account.
3. Add these repository secrets (*Settings → Secrets and variables →
   Actions*):

   | secret | value |
   |---|---|
   | `AZURE_TENANT_ID` | the directory (tenant) ID |
   | `AZURE_CLIENT_ID` | the application (client) ID |
   | `AZURE_CLIENT_SECRET` | the client secret |
   | `TRUSTED_SIGNING_ENDPOINT` | the account's endpoint, `https://<region>.codesigning.azure.net` |
   | `TRUSTED_SIGNING_ACCOUNT` | the account name |
   | `TRUSTED_SIGNING_PROFILE` | the certificate profile name |

4. Run a release. The step *Sign with Azure Artifact Signing* appears in
   the Windows job and the notes say "signed by CN=…".

### A `.pfx` file

Encode the file (`[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`)
into the secret `CODE_SIGNING_PFX_BASE64`, its password into
`CODE_SIGNING_PFX_PASSWORD`, and optionally a timestamp server into
`CODE_SIGNING_TIMESTAMP_URL` (default `http://timestamp.digicert.com`).
The workflow writes the file to the runner's temporary directory, signs
with `signtool` from the Windows SDK and deletes it.

### Cloud signing through a CA tool, or SignPath

These need the CA's own CLI or SignPath's actions in the workflow; add
them to the Windows job between the artefact check and the signature
check, so that the verification and the release notes cover them.

## After the first signed release

- Keep the same certificate (or the same validated identity, for Azure)
  for every release: reputation follows it.
- Expect the warning to persist for the first releases; it fades with
  clean downloads. Nothing in the executable itself can speed that up.
- When a user reports the warning, point them at *More info → Run
  anyway* and at the SHA-256 in `SHA256SUMS.txt`, and ask them to use the
  *Report that this app is safe* link on the warning: those reports feed
  the reputation too.
