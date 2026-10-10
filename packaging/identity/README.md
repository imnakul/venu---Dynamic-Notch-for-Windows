# Optional sparse identity package

Venu's Windows toast listener uses the supported `UserNotificationListener` API. Windows makes that API available only to an app with package identity, the `userNotificationListener` capability, and permission granted by the user. The regular portable build and Inno Setup installer remain unpackaged.

This directory prepares a sparse MSIX identity package that points at an existing `venu.exe`. It does not contain the app, install the package, import a certificate, or grant Windows notification permission. The publisher and identity name must come from the maintainer's existing trusted signing setup; no certificate or publisher is embedded here.

## Build a signed identity package

Use Windows 10 build 19041 or newer, install the Windows SDK tools `MakeAppx.exe` and `SignTool.exe`, and make them available on `PATH`. Build Venu and install or extract it to the directory that should receive package identity. The signing certificate must already be available in the current user's `Personal` certificate store with its private key.

```powershell
$appDirectory = "$env:LOCALAPPDATA\Programs\Venu"
$certificateThumbprint = "<thumbprint of the existing trusted signing certificate>"

./packaging/identity/build-sparse-identity.ps1 `
  -ApplicationDirectory $appDirectory `
  -IdentityName "<maintainer-assigned package identity name>" `
  -CertificateThumbprint $certificateThumbprint `
  -OutputPath "$PWD/dist/venu-identity.msix"
```

The script takes the package publisher from the selected certificate's subject, signs the package, verifies the signature, and writes the result only after both steps pass. It does not create or trust a certificate. The output is a signed identity package, not a replacement for the regular installer.

## Register identity and grant permission

After reviewing the package and confirming that the signing certificate is trusted on the machine, register it for the existing app directory:

```powershell
Add-AppxPackage `
  -Path "$PWD/dist/venu-identity.msix" `
  -ExternalLocation "$env:LOCALAPPDATA\Programs\Venu"
```

Start Venu, open **Settings > Notch > Notifications**, choose **Request or retry Windows permission**, and accept the Windows prompt. Venu asks only after that explicit action. Disconnecting in Settings removes the listener; the OS notification center itself remains unchanged.

To remove the identity registration, use the package identity name supplied to the build script:

```powershell
Get-AppxPackage -Name "<maintainer-assigned package identity name>" | Remove-AppxPackage
```

Removing the identity registration does not uninstall Venu from its external location. The existing Inno Setup and portable distribution workflows are unchanged. Production identity packaging remains dependent on the project's trusted publisher and signing configuration.
