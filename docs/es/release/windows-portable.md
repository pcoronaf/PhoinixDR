# Versión portable para Windows

## Requisito

> **REL-001.** La versión portable estándar para Windows DEBERÁ
> distribuirse como un único ejecutable y NO DEBERÁ requerir instalación ni
> dependencias de PHOINIX instaladas por separado. Puede apoyarse en
> componentes del sistema operativo incluidos en las versiones de Windows
> admitidas, incluido WebView2.

Las versiones de Windows admitidas son Windows 10 (21H2 o posterior) y
Windows 11, de 64 bits. Ambas incluyen el entorno WebView2 Evergreen como
componente del sistema operativo.

## Cómo se cumple el requisito

| artefacto | contenido | dependencias |
|---|---|---|
| `PhoinixDR-<versión>-windows-x64-portable.exe` | la aplicación de escritorio: la capa Tauri con el front-end React incrustado en tiempo de compilación y todo el motor de recuperación enlazado dentro | WebView2 (parte de Windows), las DLL del entorno de Visual C++ que vienen con Windows |
| `phoinix-windows-x64.exe` | la aplicación de línea de comandos | ninguna más allá del propio Windows |

- Los recursos del front-end se compilan dentro del ejecutable; el
  ejecutable no abre archivos contiguos y no necesita un directorio
  `resources`. `bundle.resources` permanece vacío en `tauri.conf.json`.
- No se instala nada. El ejecutable puede ejecutarse desde una carpeta de
  descargas, una memoria USB o un recurso compartido de red. Solo escribe
  lo que el usuario pide: los archivos recuperados en el destino elegido,
  los informes en la ruta elegida y las sesiones de escaneo en el
  directorio de datos de aplicación local del usuario
  (`%LOCALAPPDATA%\org.phoinixdr.desktop`), que se crea en el primer uso y
  puede borrarse en cualquier momento.
- No se requiere ninguna biblioteca, servicio, controlador ni entorno de
  ejecución de PHOINIX junto al ejecutable. Todos los motores de sistemas
  de archivos y lectores de contenedores de imagen son código Rust nativo
  enlazado en el binario (ADR-0004, ADR-0013).
- WebView2 es el único entorno que necesita el ejecutable de escritorio.
  Cuando falta (una instalación de Windows 10 anterior a 21H2 sin
  actualizaciones), el ejecutable informa del entorno ausente y remite al
  Evergreen Bootstrapper de Microsoft; los instaladores MSI/NSIS
  opcionales, que no son la versión portable estándar, lo descargan
  automáticamente (`webviewInstallMode: downloadBootstrapper`).
- Leer discos físicos requiere el mismo privilegio que Windows exige a
  cualquier herramienta que abra `\\.\PhysicalDriveN`: ejecutar el
  ejecutable como administrador. Las imágenes de disco no necesitan
  elevación.

## Verificación

El flujo de trabajo de publicación (`.github/workflows/release.yml`)
compila ambos ejecutables en `windows-latest` (el de escritorio mediante
la CLI de Tauri, cuya compilación de producción incrusta el front-end),
ejecuta `phoinix.exe --version` y `phoinix.exe inspect` sobre un fixture,
falla si la compilación de escritorio produce algo más que el único
ejecutable en su directorio de salida, y falla si el ejecutable no contiene
el paquete del front-end (una compilación de desarrollo buscaría un
servidor local en su lugar). El `SHA256SUMS.txt` publicado permite a los
usuarios verificar lo que han descargado:

```powershell
Get-FileHash .\PhoinixDR-<versión>-windows-x64-portable.exe -Algorithm SHA256
```

## Versión

El ejecutable de escritorio lleva el número de versión en su nombre de
archivo (`PhoinixDR-0.1.2-windows-x64-portable.exe` para la versión 0.1.2)
y en su recurso de versión de Windows: clic derecho sobre el archivo,
*Propiedades*, *Detalles* muestra *Nombre del producto* PhoinixDR, *Versión
del archivo* y *Versión del producto*. La compilación de Tauri obtiene el
recurso de `tauri.conf.json`, y el flujo de publicación se niega a publicar
cuando la etiqueta, la versión del espacio de trabajo en `Cargo.toml` y el
recurso de versión no coinciden. La aplicación muestra la misma versión
junto al autor en su barra superior.

## Otras plataformas

Las versiones para Linux son un archivo tar con los dos ejecutables
(`phoinix`, `phoinix-desktop`); el binario de escritorio necesita los
paquetes WebKitGTK 4.1 de la distribución, que son el equivalente de la
dependencia de WebView2. Son compilaciones de conveniencia: Windows es la
plataforma para la que se redactó el requisito portable.
