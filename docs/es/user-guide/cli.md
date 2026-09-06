# Guía de la línea de comandos

`phoinix` es la cara de línea de comandos de PhoinixDR. Todos los comandos
son de solo lectura respecto a la fuente: lo único que escriben son los
archivos recuperados, los informes y la salida JSON que usted pida. Los
mensajes del programa están en inglés.

```text
phoinix [OPCIONES] <COMANDO>

Comandos:
  devices     Lista los dispositivos de bloques visibles para este proceso
  inspect     Identifica la tabla de particiones y los sistemas de archivos de un dispositivo o imagen
  verify      Calcula el hash de una fuente y lo compara con los hashes almacenados en su contenedor (E01)
  partitions  Busca volúmenes por sus estructuras, con independencia de la tabla de particiones
  scan        Escanea una fuente en busca de archivos recuperables y evalúa su salud
  explain     Explica la evidencia detrás de la salud de recuperación de un candidato
  recover     Recupera candidatos en otro sistema de archivos y los verifica
  ntfs        Comandos del lector NTFS nativo (info, ls, record, extract)
  read        Lee bytes en bruto de una fuente (comando de desarrollo/depuración)

Opciones:
  -v, --verbose...   -v info, -vv debug, -vvv trace (por stderr)
  -V, --version      Muestra la versión
```

`--version` muestra la compilación: `phoinix 0.1.0 by @pcoronaf`.

## Fuentes

Una *fuente* es la ruta de un dispositivo o un archivo de imagen:

| fuente | ejemplo |
|---|---|
| disco físico (Windows) | `\\.\PhysicalDrive1` (ejecutar como administrador) |
| dispositivo de bloques (Linux) | `/dev/sdb` (ejecutar con `sudo`) |
| imagen RAW / dd | `disk.img`, `stick.dd` |
| RAW dividida | `disk.001` (cualquier segmento; los demás se localizan por nombre) |
| EWF / E01 | `case.E01` (se siguen `E01`…`E99`, `EAA`…; también SMART `.s01`) |
| VHD, VHDX, VMDK | `disk.vhd`, `disk.vhdx`, `disk.vmdk` |

Los contenedores se reconocen por su contenido, así que un `.img` que en
realidad es un E01 se abre como E01. Véase
[contenedores de imagen](../../images/containers.md) (en inglés).

### Elegir el volumen

`scan`, `explain`, `recover` y los comandos `ntfs` trabajan sobre un
volumen:

| opción | significado |
|---|---|
| (ninguna) | la primera partición con un sistema de archivos admitido, o toda la fuente cuando no tiene tabla de particiones |
| `--partition N` | la partición `N` de la tabla (empezando en 1, como imprime `inspect`) |
| `--lost N` | el candidato `N` de `phoinix partitions`, montado virtualmente con sus reparaciones |
| `--at DESPLAZAMIENTO [--length BYTES]` | un rango de bytes explícito |

## Flujo de trabajo

### 1. Localizar la fuente

```bash
phoinix devices              # discos visibles para este proceso (elevado en Windows)
phoinix devices --partitions # incluye los nodos de partición
phoinix devices --json
```

### 2. Inspeccionarla

```bash
phoinix inspect disk.img
phoinix inspect case.E01           # añade una sección "Image container"
phoinix inspect disk.img --json
phoinix inspect disk.img --fingerprint   # SHA-256 del primer y del último MiB
```

La salida lista la tabla de particiones, sus diagnósticos y, para cada
volumen, el sistema de archivos detectado con la evidencia de la
detección.

### 3. Verificar una imagen (opcional)

```bash
phoinix verify case.E01        # MD5, SHA-1, SHA-256; los compara con los hashes almacenados
phoinix verify disk.vmdk       # sin hash almacenado: los hashes calculados documentan la fuente
phoinix verify case.E01 --json
```

El código de salida es distinto de cero cuando un hash almacenado no
coincide.

### 4. Escanear

```bash
phoinix scan disk.img                   # archivos borrados a través de los metadatos del sistema de archivos
phoinix scan disk.img --deep            # además talla el espacio no asignado por firma
phoinix scan disk.img --deep --carve-types jpeg,pdf,docx
phoinix scan disk.img --carve-all       # talla todo el volumen (también el espacio asignado)
phoinix scan disk.img --carve-only      # omite el escaneo de metadatos
phoinix scan disk.img --min-health good --name factura
phoinix scan disk.img --no-content      # más rápido; reduce la confianza de la evaluación
phoinix scan disk.img --json
```

Cada fila muestra la referencia del candidato (`ID`), el nombre, el
tamaño, la **probabilidad de recuperación** con su categoría, la
**confianza de la evaluación** y la ruta original. Los archivos tallados se
referencian como `c<desplazamiento>`. Categorías:

| probabilidad | categoría |
|---|---|
| 95–100 | Excellent (excelente) |
| 80–94 | Very good (muy buena) |
| 60–79 | Good (buena) |
| 35–59 | Poor (baja) |
| 1–34 | Very poor (muy baja) |
| 0 | Unrecoverable (irrecuperable) |

Opciones del escaneo profundo: `--carve-align` (512 por defecto; `1`
prueba cada byte y es lento), `--carve-min-size`, `--carve-threads`,
`--carve-signatures archivo.json` para sus propias firmas (véase
[escaneo profundo](../../carving/deep-scan.md), en inglés).

Cuando stderr es un terminal, un escaneo profundo informa de sus dos etapas
de tallado: la búsqueda de cabeceras (bytes escaneados, coincidencias hasta
el momento) y después el examen de cada coincidencia (coincidencias
examinadas, archivos ensamblados). La segunda etapa lee de nuevo la fuente,
coincidencia a coincidencia, y en un volumen grande con muchas coincidencias
puede durar más que la búsqueda.

### 5. Entender un candidato

```bash
phoinix explain disk.img 64
phoinix explain disk.img c1048576
phoinix explain disk.img 64 --json
```

`explain` imprime cada razón detrás de los dos números: la validez de los
metadatos, si la disposición es conocida, cuántos clústeres están
asignados a otros archivos, qué encontró el examen del contenido, y
diagnósticos como «Layout recovered from journal transaction 9». Léalo
antes de confiar en una cifra. La redacción es deliberada: «allocated to
active filesystem data» significa que la reutilización está demostrada,
no que todos los bytes hayan desaparecido.

### 6. Particiones perdidas

```bash
phoinix partitions disk.img              # busca estructuras de volúmenes en toda la fuente
phoinix partitions disk.img --no-verify  # más rápido: no abre los candidatos con sus motores
phoinix scan disk.img --lost 2
phoinix recover disk.img --lost 2 64 --output /mnt/recuperacion
```

No se escribe nada en la tabla de particiones. Un candidato cuyo sector
de arranque primario está destruido se monta con su copia de seguridad
superpuesta en memoria.

### 7. Recuperar

```bash
phoinix recover disk.img 64 65 c1048576 --output /mnt/recuperacion
phoinix recover disk.img 64 --output /mnt/recuperacion --preserve-tree
phoinix recover disk.img 64 --output /mnt/recuperacion --no-timestamps --no-hash --overwrite
```

Reglas que aplica el escritor:

- el destino no debe estar en el disco de origen
  (`--allow-source-destination` es una anulación para expertos que puede
  destruir los datos que está recuperando);
- los archivos existentes nunca se sobrescriben salvo con `--overwrite`;
- cada archivo escrito se resume con SHA-256 y se notifica como completo o
  `PARTIAL`; una recuperación parcial hace que el comando termine con
  código distinto de cero.

### 8. Informe

```bash
phoinix recover case.E01 64 65 --output D:\recuperados --report D:\recuperados\informe.html \
    --case-number 2026-017 --evidence-number HDD-3 --examiner "J. Pérez" --verify-source
```

El informe (`.html`, `.md` o `.json` según la extensión) registra la
versión de la herramienta, los metadatos del caso (los campos no indicados
se toman de la cabecera de adquisición del E01), la fuente y su
contenedor, los hashes almacenados y calculados con `--verify-source`, y
cada archivo con su salud en el momento de la recuperación, su ruta de
salida y su SHA-256. Véase
[informes](../../images/containers.md#case-metadata-and-reports) (en inglés).

## Comandos NTFS para desarrolladores

```bash
phoinix ntfs info volume.img                 # geometría, ubicación de la MFT, indicadores
phoinix ntfs ls volume.img --all --system    # todos los registros MFT, incluidos borrados y del sistema
phoinix ntfs record volume.img 5 --hex       # un registro con sus atributos y un volcado hexadecimal
phoinix ntfs extract volume.img --record 64 --stream Zone.Identifier --output salida.bin
phoinix read disk.img --offset 512 --length 512 --hex
```

## Registro

`-v` imprime en stderr lo que hace el motor a nivel `info`, `-vv` añade
`debug` y `-vvv` `trace`. Las líneas indican la imagen o el dispositivo
abierto, el sistema de archivos encontrado, los registros recorridos, la
pasada de tallado y los recuentos, y nunca contienen contenido recuperado.
La aplicación de escritorio muestra los mismos registros en directo en el
modo **Advanced**, junto con el comando `phoinix` equivalente a lo que está
haciendo, de modo que un informe puede llevar cualquiera de los dos. La
variable `RUST_LOG` sustituye el filtro (`RUST_LOG=phoinix_fs_ntfs=trace`).

## Códigos de salida

| código | significado |
|---|---|
| 0 | éxito (para `recover`: todos los archivos completos; para `verify`: los hashes coinciden o no hay ninguno almacenado) |
| 1 | un error, una recuperación fallida o parcial, o un hash que no coincide; el mensaje indica la causa |

## JSON

Todos los comandos aceptan `--json`. Los objetos usan claves en
`snake_case`, los tamaños son recuentos de bytes, las fechas son ISO-8601
UTC y los nombres de sistemas de archivos van en kebab-case (`ntfs`,
`fat32`, `ex-fat`, `ext`). La salida de `scan` es
`{ "filesystem": …, "candidates": [ … ], "carving": … }`; cada candidato
lleva su `filesystem_object` (la referencia estable que usan `explain` y
`recover`), su `evidence` y su `health`.
