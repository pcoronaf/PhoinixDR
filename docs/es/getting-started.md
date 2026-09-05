# Primeros pasos

## Descarga (Windows)

1. Abra la [última versión publicada](https://github.com/pcoronaf/PhoinixDR/releases/latest).
2. Descargue `PhoinixDR-<versión>-windows-x64-portable.exe` (aplicación de
   escritorio; el nombre del archivo lleva el número de versión, por ejemplo
   `PhoinixDR-0.1.2-windows-x64-portable.exe`)
   y, si quiere la línea de comandos, `phoinix-windows-x64.exe`.
3. Opcionalmente, compruebe la descarga con `SHA256SUMS.txt`:

   ```powershell
   Get-FileHash .\PhoinixDR-<versión>-windows-x64-portable.exe -Algorithm SHA256
   ```

4. Ejecútelo. No se instala nada; el ejecutable solo necesita Windows 10
   (21H2 o posterior) o Windows 11, cuyo entorno WebView2 utiliza. Para
   escanear discos físicos, ejecútelo como administrador. Véase
   [Versión portable para Windows](release/windows-portable.md).

Windows SmartScreen puede advertir sobre un ejecutable sin firmar de un
editor nuevo; el SHA-256 de la versión publicada le permite confirmar que
es el archivo publicado aquí.

## Descarga (Linux)

Descargue `phoinix-linux-x64.tar.gz`, descomprímalo y ejecute `./phoinix`
o `./phoinix-desktop`. El binario de escritorio necesita los paquetes
WebKitGTK 4.1 de la distribución (`libwebkit2gtk-4.1-0`, `libgtk-3-0`,
`libayatana-appindicator3-1`). Leer dispositivos requiere `sudo`.

## Compilar desde el código fuente

PhoinixDR es un espacio de trabajo de Cargo sobre Rust estable (edición
2024, Rust 1.85 o posterior). La aplicación de escritorio es una capa
Tauri 2 con un front-end React/TypeScript y necesita Node.js 22.

### Windows

1. Instale [Rust](https://rustup.rs) (la cadena de herramientas
   `x86_64-pc-windows-msvc`) y la carga de trabajo *Desarrollo de
   escritorio con C++* de Visual Studio Build Tools, que aporta el enlazador
   MSVC.
2. Instale [Node.js 22](https://nodejs.org) (LTS).
3. Instale el [entorno WebView2](https://developer.microsoft.com/microsoft-edge/webview2/)
   si `Get-AppxPackage *WebView2*` o Windows 10 21H2 no lo proporcionan ya.
4. Compile:

   ```powershell
   git clone https://github.com/pcoronaf/PhoinixDR.git
   cd PhoinixDR
   cargo build --release                 # target\release\phoinix.exe
   cd apps\desktop
   npm ci
   npx tauri build --no-bundle           # compilación de producción de la aplicación de escritorio
   # apps\desktop\src-tauri\target\release\phoinix-desktop.exe (portable)
   npm run tauri dev                     # ventana de desarrollo con recarga en caliente
   ```

   Compile la aplicación de escritorio con la CLI de Tauri, como arriba. Un
   `cargo build --release` directo dentro de `src-tauri` produce un binario
   de *desarrollo* que espera el servidor de Vite en `localhost:1420` y
   muestra «localhost refused to connect» al ejecutarse solo; la CLI activa
   la característica de producción que incrusta el front-end. `npx tauri
   build` sin `--no-bundle` genera además instaladores MSI/NSIS en
   `src-tauri\target\release\bundle`; son opcionales.

### Linux (Debian/Ubuntu)

```bash
sudo apt install build-essential curl pkg-config libssl-dev \
    libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
curl https://sh.rustup.rs -sSf | sh
git clone https://github.com/pcoronaf/PhoinixDR.git
cd PhoinixDR
cargo build --release                    # target/release/phoinix
cd apps/desktop && npm ci && npx tauri build --no-bundle
# apps/desktop/src-tauri/target/release/phoinix-desktop
```

### Pruebas

```bash
cargo test --workspace        # pruebas unitarias y de integración sobre los corpus de fixtures
cd apps/desktop && npm test   # pruebas del front-end
```

Los fixtures de `tests/fixtures` están en el repositorio; los scripts de
`tests/generated` los reconstruyen (necesitan mkntfs, mkfs.fat, mkfs.exfat,
mke2fs, ewfacquire y qemu-img, y permisos de root para los montajes loop).

## Primera recuperación

```bash
phoinix inspect stick.img            # qué hay en la fuente
phoinix scan stick.img               # archivos borrados con su salud de recuperación
phoinix explain stick.img 64         # por qué un archivo obtiene esa puntuación
phoinix recover stick.img 64 --output ~/recuperados
```

O abra la aplicación de escritorio, elija *Disk image*, seleccione el
archivo, mantenga *Quick Scan* y pulse *Recover* en las filas que quiera.
La [guía de escritorio](user-guide/desktop.md) y la
[guía de línea de comandos](user-guide/cli.md) cubren todas las opciones.

## Siguientes pasos

- [Preguntas frecuentes](faq.md): por qué un archivo muestra 0 %, por qué
  una memoria USB necesita permisos de administrador, qué significa
  «asignado».
- [Modelo de salud](../recovery/health-model.md) (en inglés): cómo se
  calculan la probabilidad y la confianza.
- [Arquitectura](../architecture/overview.md) y
  [registros de decisiones](../decisions/README.md) (en inglés).
