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

### ¿Por qué necesito permisos de administrador? ¿Hay alternativa?

Leer `\\.\PhysicalDriveN` (Windows) o `/dev/sdX` (Linux) es una operación
privilegiada en ambos sistemas, así que escanear un dispositivo
directamente implica ejecutar PhoinixDR como administrador (o con `sudo`)
y recuperar en un solo paso. No hace falta iniciarlo así: cuando un
dispositivo aparece como no accesible, el botón **Restart as
administrator** pide la elevación al sistema y reinicia PhoinixDR elevado. La alternativa es crear primero una imagen
del dispositivo con una herramienta de adquisición y abrir la imagen en
PhoinixDR, que entonces no necesita elevación; la necesita la herramienta
de adquisición. Véase
[dos formas de recuperar](getting-started.md#dos-formas-de-recuperar).

### La aplicación de escritorio no arranca en Windows.

Necesita el entorno WebView2, que forma parte de Windows 10 21H2+ y
Windows 11. En una instalación antigua o recortada, instale el WebView2
Evergreen Bootstrapper de Microsoft. La línea de comandos `phoinix.exe` no
tiene esa dependencia.

### ¿Qué versión tengo?

El ejecutable de escritorio lleva la versión en su nombre de archivo
(`PhoinixDR-0.1.2-windows-x64-portable.exe`) y en sus propiedades (pestaña
*Detalles*), y la muestra junto al autor en la barra superior. La línea de
comandos la imprime con `phoinix --version`.

### SmartScreen o un antivirus avisan sobre el ejecutable.

Los ejecutables leen discos en bruto, algo que algunas heurísticas
señalan, y el editor es nuevo. Compare el SHA-256 con el `SHA256SUMS.txt`
de la versión publicada, o compile desde el código fuente.

### ¿Puedo recuperar desde un SSD?

Raramente, y PhoinixDR lo avisa antes de escanear. Windows indica al SSD
qué bloques ocupaba un archivo borrado y la unidad los descarta (TRIM) en
segundos; desde entonces se leen como ceros aunque el registro del sistema
de archivos siga describiendo el archivo a la perfección. Ninguna
herramienta puede leer más allá de eso. Recuperar desde un SSD solo
funciona cuando TRIM no estaba en vigor: una carcasa USB que no lo
transmite, TRIM desactivado o datos perdidos por un reformateo antes de que
TRIM actuara. La página de configuración del escaneo muestra un aviso para
las fuentes de estado sólido, y el contenido de cada candidato se muestrea
en busca de ceros con independencia de la opción *Examine content*, de modo
que un archivo descartado muestra una probabilidad baja con el motivo
«zero-filled» en lugar de un registro de aspecto intacto.

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

### ¿Qué hace la casilla Advanced?

Muestra el detalle técnico que hay detrás de la interfaz sin cambiar el
escaneo ni la recuperación: el comando `phoinix` equivalente al escaneo
actual y al archivo seleccionado, un registro en directo de lo que hace el
motor, la referencia del candidato en el sistema de archivos, las
comprobaciones de validación de estructura y un bloque de diagnóstico.
Véase [Modo avanzado](user-guide/desktop.md#modo-avanzado).

### ¿Cómo obtengo un registro para un informe de error?

Marque **Advanced** antes de escanear; la página de escaneo muestra
entonces el registro del motor con un botón *Copy log*, y la página de
resultados conserva *Copy scan log*. En la línea de comandos, ejecute el
mismo comando con `-vv` y adjunte stderr. En ambos casos el registro
describe estructuras y recuentos, nunca el contenido de sus archivos.
Añada la salida de `phoinix explain` del archivo en cuestión y, cuando
pueda, una imagen del volumen.

### ¿Basta una pasada de sobreescritura para borrar un disco?

Sí, en cualquier disco fabricado en este siglo. Los esquemas de varias
pasadas (DoD 5220.22-M, las 35 pasadas de Gutmann) proceden de las
codificaciones de disco de los años noventa; el epílogo del propio Gutmann
califica el método completo de «pointless for any drive», y NIST SP 800-88
afirma que una sobreescritura protege los discos fabricados después de
2001 tanto frente a ataques por teclado como de laboratorio. PhoinixDR
ofrece los métodos de varias pasadas porque algunas políticas todavía los
exigen, y muestra un aviso que enlaza
[El mito de Gutmann](https://www.pcoronaf.com/2014/07/el-mito-de-gutmann.html)
al elegir uno.

### ¿Puede PhoinixDR borrar el espacio libre de un disco sin borrar el disco?

Sí, desde M12.2. Al eliminar un archivo se liberan sus clústeres y los
datos quedan ahí, que es justo lo que aprovecha la recuperación; un
borrado de espacio libre sobrescribe esos clústeres y nada más. En la
página de borrado de un disco elija *Only the free space of partition N*
(el volumen se lee con el mapa de asignación del motor de recuperación,
necesita derechos de administrador y no debe estar en uso), opcionalmente
con el *slack* final de cada archivo; o elija un volumen en uso en la
segunda lista de la página de borrado, y PhoinixDR lo llena a través del
sistema de archivos sin derechos de administrador y elimina los archivos de
relleno al final. En ambos casos un escaneo posterior opcional examina el
resultado con los motores de recuperación y el certificado anota lo que
encontró. Lo que no se alcanza (archivos pequeños guardados dentro de los
registros MFT de NTFS, copias en el diario, instantáneas, celdas
reasignadas de la memoria flash) se indica en la página y en el
certificado.

### ¿Puede PhoinixDR borrar de forma segura un SSD?

Solo a nivel de software. Una sobreescritura llega a los bloques lógicos
del SSD, no a sus celdas físicas: el controlador reasigna las escrituras y
las celdas de sobreaprovisionamiento o retiradas nunca se tocan. Eso basta
contra el software de recuperación (*Clear* de NIST 800-88). Un *purge*
real necesita el comando de sanitización del propio dispositivo (ATA Secure
Erase, NVMe Sanitize), previsto para M12.4, o la destrucción física. La
página de borrado lo advierte siempre que el objetivo es de estado sólido.

### ¿Cómo se construyó PhoinixDR?

Con una amplia asistencia de IA y una ingeniería deliberada. Lea la
[Declaración de desarrollo](about/development-declaration.md),
[Sí, PHOINIX está «vibecodeado»](about/vibecoded.md) y
[De dónde viene PHOINIX](about/origin.md).
