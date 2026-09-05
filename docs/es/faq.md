# Preguntas frecuentes

### ¿PhoinixDR escribe en mi disco?

No. La capa de bloques no tiene primitiva de escritura, así que ningún
componente del motor de recuperación puede modificar una fuente (ADR-0002,
ADR-0007). Lo único que se escribe son los archivos recuperados, los
informes y las sesiones, y el escritor de recuperación rechaza un destino
situado en el disco de origen.

### ¿Por qué un archivo muestra 0 % / «Unrecoverable»?

No se pudo localizar ninguna extensión de su contenido: el sistema de
archivos borró la disposición del archivo al eliminarlo y no sobrevivió
ninguna copia en el diario (ext2, un diario ext4 que dio la vuelta), o el
registro está dañado. El nombre y las marcas de tiempo se muestran igual
porque son evidencia de que el archivo existió. Un **escaneo profundo**
puede encontrar el contenido por firma; los hallazgos tallados que empiezan
donde un candidato de metadatos espera sus datos se fusionan con él.

### El escaneo de mi memoria USB muestra «Unrecoverable» en todo menos en un archivo.

En volúmenes FAT32 grandes, el controlador de Windows borra la mitad alta
del número del primer clúster al eliminar. PhoinixDR infiere el inicio
real a partir de los clústeres libres y su contenido (`explain` imprime
«start was inferred»). Asegúrese de usar una versión actual; la corrección
se incluye desde la continuación del hito M7.

### ¿Qué significa «allocated to active filesystem data»?

Los clústeres que usaba un archivo borrado están ahora marcados como en
uso por otros archivos. Los bytes antiguos pueden seguir ahí o no; PhoinixDR
informa de la asignación como evidencia de reutilización y reduce la
probabilidad en consecuencia, pero nunca afirma «sobrescrito» sin
pruebas.

### ¿Probabilidad frente a confianza?

La **probabilidad** (likelihood) estima la posibilidad de que la
recuperación devuelva los bytes originales. La **confianza** (confidence)
indica cuánta evidencia respalda esa estimación: un escaneo sin examen de
contenido, un archivo sin validador estructural o un medio desconocido
reducen la confianza sin cambiar la probabilidad. Véase el
[modelo de salud](../recovery/health-model.md) (en inglés).

### ¿Por qué necesito permisos de administrador?

Leer `\\.\PhysicalDriveN` (Windows) o `/dev/sdX` (Linux) es una operación
privilegiada en ambos sistemas. Las imágenes de disco no necesitan
elevación.

### La aplicación de escritorio no arranca en Windows.

Necesita el entorno WebView2, que forma parte de Windows 10 21H2+ y
Windows 11. En una instalación antigua o recortada, instale el WebView2
Evergreen Bootstrapper de Microsoft. La línea de comandos `phoinix.exe` no
tiene esa dependencia.

### SmartScreen o un antivirus avisan sobre el ejecutable.

Los ejecutables leen discos en bruto, algo que algunas heurísticas
señalan, y el editor es nuevo. Compare el SHA-256 con el `SHA256SUMS.txt`
de la versión publicada, o compile desde el código fuente.

### ¿Puedo recuperar desde un SSD?

Sí, con una salvedad: tras un borrado, la unidad puede haber descartado
(TRIM) los bloques, en cuyo caso se leen como ceros. PhoinixDR no tiene
evidencia sobre el estado de la NAND, así que avisa y reduce la confianza
en lugar de fingir que lo sabe. El contenido lleno de ceros de un tipo
reconocido se notifica como contradicción con su formato.

### ¿Qué sistemas de archivos e imágenes se admiten?

NTFS, FAT12/16/32, exFAT y ext2/3/4 con recuperación de borrados;
cualquier otro volumen puede escanearse en profundidad (tallado). Fuentes:
discos físicos, imágenes RAW/dd, RAW divididas, E01/E01 divididas/SMART,
VHD, VHDX y VMDK. HFS+, APFS, XFS, Btrfs y RAID/LVM están en la hoja de
ruta.

### ¿El archivo recuperado es idéntico al original?

Cuando la disposición es conocida y todos los clústeres están libres, sí,
y el SHA-256 de la salida de recuperación le permite compararlo con
cualquier hash que tenga. Cuando algunos clústeres se reutilizaron, el
archivo se escribe tal como está ahora, marcado con su evidencia de
asignación, para que pueda juzgarlo.

### ¿Dónde se guardan las sesiones?

En el directorio de datos de aplicación local del usuario
(`%LOCALAPPDATA%\org.phoinixdr.desktop` en Windows,
`~/.local/share/org.phoinixdr.desktop` en Linux). Contienen los resultados
del escaneo y la evidencia, nunca el contenido de los archivos. Borre el
directorio para eliminarlas.

### ¿Puede PhoinixDR reparar una tabla de particiones o un sistema de archivos?

No. Las particiones perdidas se montan virtualmente y se recupera desde
ellas; la tabla nunca se escribe (ADR-0011). Las herramientas de
reparación cambian el disco del que intenta recuperar y quedan fuera del
alcance por diseño.

### ¿Cómo se construyó PhoinixDR?

Con una amplia asistencia de IA y una ingeniería deliberada. Lea la
[Declaración de desarrollo](about/development-declaration.md),
[Sí, PHOINIX está «vibecodeado»](about/vibecoded.md) y
[De dónde viene PHOINIX](about/origin.md).
