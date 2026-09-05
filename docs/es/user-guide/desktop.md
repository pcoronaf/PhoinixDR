# Guía de la aplicación de escritorio

La aplicación de escritorio de PhoinixDR sigue los mismos pasos que la
línea de comandos: elegir una fuente, escanear, entender lo encontrado,
previsualizar, recuperar. Nunca escribe en la fuente. La interfaz está en
inglés; esta guía indica entre paréntesis los textos que verá en pantalla.

## Inicio

<figure>
  <img src="../../user-guide/images/01-home.png" alt="Página de inicio de PhoinixDR" width="960">
  <figcaption>La página de inicio: elija un disco físico, un dispositivo extraíble o una imagen de disco; debajo se listan las sesiones recientes.</figcaption>
</figure>

- **Windows:** ejecute `PhoinixDR-<versión>-windows-x64-portable.exe`. No se instala
  nada (véase [Versión portable para Windows](../release/windows-portable.md)).
  Para escanear un disco físico o una memoria USB, haga clic derecho en el
  ejecutable y elija *Ejecutar como administrador*; las imágenes de disco
  no lo necesitan.
- **Linux:** ejecute `phoinix-desktop`; los dispositivos físicos necesitan
  `sudo` o una regla udev que conceda acceso de lectura a los discos.

La página de inicio muestra la versión y el autor en la barra superior y
lista las sesiones de escaneo recientes.

## Dos formas de recuperar

PhoinixDR puede trabajar sobre el propio dispositivo o sobre una imagen de
este. Elija una antes de empezar; el resto de esta guía vale para ambas:

1. **Directamente desde el dispositivo, en un solo paso.** Elija *Physical
   disk* o *Removable device*; cuando el dispositivo aparezca como no
   accesible, pulse **Restart as administrator** y acepte el aviso del
   sistema (o inicie PhoinixDR elevado usted mismo: Windows clic derecho,
   *Ejecutar como administrador*; Linux `sudo`). Después escanee y recupere. Es lo más rápido; PhoinixDR solo lee del
   dispositivo.
2. **Desde una imagen de disco.** Cree primero una imagen del dispositivo
   con una herramienta de adquisición (FTK Imager o Arsenal Image Mounter
   en Windows, `dd` o `ewfacquire` en Linux) y abra después el archivo de
   imagen en PhoinixDR con *Disk image*. PhoinixDR no necesita elevación
   para ello; la herramienta de adquisición sí, porque lee el mismo
   dispositivo en bruto. Es la vía recomendada para un disco que falla,
   para todo lo que quizá tenga que examinar de nuevo y para el trabajo
   forense (los hashes de un E01 se verifican y se recogen en el informe).

Las dos vías dan los mismos resultados en un dispositivo sano. En ambas,
recupere a un disco distinto del que está recuperando.

## 1. Elegir una fuente

<figure>
  <img src="../../user-guide/images/02-devices.png" alt="Selector de dispositivos con un disco no accesible y el botón Restart as administrator" width="960">
  <figcaption>El selector de dispositivos. Un disco que no puede abrirse aparece atenuado y el aviso ofrece reiniciar con permisos de administrador.</figcaption>
</figure>

| opción | qué lista |
|---|---|
| **Physical disk** (disco físico) | unidades internas y SSD |
| **Removable device** (dispositivo extraíble) | memorias USB, tarjetas SD, discos externos |
| **Disk image** (imagen de disco) | un archivo: RAW/dd, RAW dividida, E01 (y E01 dividida), SMART, VHD, VHDX, VMDK |

Un dispositivo que no puede abrirse aparece atenuado como *Not accessible
from this process* (no accesible desde este proceso), y un aviso ofrece
**Restart as administrator** (reiniciar como administrador): PhoinixDR
pide la elevación al sistema (el aviso UAC de Windows, el diálogo de
contraseña de polkit en Linux) e inicia una copia elevada de sí mismo. En
Windows la ventana actual se cierra en cuanto arranca la nueva; en Linux
permanece abierta hasta que aparece la nueva ventana. Si rechaza el aviso,
la ventana actual sigue como estaba. El mismo aviso aparece en la página de
inicio cuando no se puede listar ningún dispositivo. También puede iniciar
usted mismo el ejecutable con *Ejecutar como administrador* o `sudo`. Las
imágenes de disco nunca necesitan elevación.

## 2. Configuración del escaneo

<figure>
  <img src="../../user-guide/images/03-setup.png" alt="Página de configuración del escaneo" width="960">
  <figcaption>Configuración del escaneo: el volumen detectado, la búsqueda de particiones perdidas, Quick o Deep Scan y el examen del contenido.</figcaption>
</figure>

La página de configuración muestra la fuente, su tabla de particiones y
cada volumen con su sistema de archivos y la confianza de la detección.

- **Image container** (contenedor de imagen; solo archivos de imagen):
  formato, variante, número de segmentos, compresión, la cabecera de
  adquisición de un E01 (número de caso, examinador, fechas, software),
  los hashes almacenados y un botón **Verify hashes** que lee la imagen
  completa y compara su MD5/SHA-1 con los almacenados.
- **Volume** (volumen): elija una partición cuando la fuente tenga varias.
- **Lost partitions** (particiones perdidas): *Search for lost partitions*
  recorre toda la fuente buscando estructuras de sistemas de archivos con
  independencia de la tabla. Un volumen encontrado puede seleccionarse y
  se monta virtualmente; cuando su sector de arranque primario está
  destruido, se usa la copia de seguridad en memoria. La tabla de
  particiones nunca se modifica.
- **Mode** (modo): *Quick Scan* lee los metadatos del sistema de archivos
  (archivos y registros borrados). *Deep Scan* además talla el espacio no
  asignado buscando archivos por firma; lee el espacio libre una vez. Los
  volúmenes sin sistema de archivos reconocido solo admiten el escaneo
  profundo.
- **Deep scan options** (opciones del escaneo profundo): tallar todo el
  volumen en lugar de solo el espacio libre; restringir los tipos de
  archivo.
- **Assessment** (evaluación): *Examine content* valida las estructuras
  de los archivos (JPEG, PNG, PDF, ZIP/DOCX, …) y eleva la confianza de la
  evaluación; desactivarlo es más rápido.

## 3. Escaneo

<figure>
  <img src="../../user-guide/images/04-scanning.png" alt="Progreso del escaneo" width="960">
  <figcaption>Escaneo en curso: fase, recuentos y velocidad, con un botón Cancel que conserva los resultados parciales.</figcaption>
</figure>

El progreso muestra la fase (metadatos, tallado), los recuentos y el
rendimiento. El escaneo puede cancelarse; los resultados parciales se
conservan.

## 4. Resultados

<figure>
  <img src="../../user-guide/images/05-results.png" alt="Resultados con el panel de evidencia" width="960">
  <figcaption>Resultados: cada candidato con su salud de recuperación; el panel de la derecha lista la evidencia detrás de los números del archivo seleccionado.</figcaption>
</figure>

Cada fila es un candidato de recuperación:

| columna | significado |
|---|---|
| name | nombre original, o un nombre sintético `carved-…` para los archivos tallados |
| health | **probabilidad** de que la recuperación devuelva los bytes originales, con su categoría (Excellent … Unrecoverable), y **confianza** en esa estimación |
| size | tamaño lógico cuando se conoce |
| type | tipo de archivo detectado o esperado |
| path | ruta original; `(uncertain)` cuando el registro del directorio se reutilizó; las etiquetas `journal` y `carved` indican cómo se encontró el archivo |

Los filtros reducen la lista por texto, categoría de salud, tipo y origen
(metadatos o tallado). Al seleccionar una fila se abre el panel de
detalle:

- la **evidencia**: cada razón positiva y negativa detrás de los dos
  números, y diagnósticos como clústeres reutilizados, transacciones del
  diario o inicios inferidos;
- una **previsualización**: las imágenes se muestran, el texto se lee, el
  resto se vuelca en hexadecimal; las previsualizaciones leen la fuente,
  nunca la escriben.

Lea la evidencia antes de confiar en un número. «Allocated to active
filesystem data» significa que los clústeres se han reutilizado, no que
todos los bytes hayan desaparecido; «Unrecoverable» significa que no se
pudo localizar ninguna extensión del contenido, en cuyo caso un escaneo
profundo todavía puede tallarlo.

<figure>
  <img src="../../user-guide/images/06-preview.png" alt="Pestaña Preview del panel de detalle" width="960">
  <figcaption>La pestaña Preview muestra imágenes y texto y vuelca en hexadecimal el resto; solo lee de la fuente.</figcaption>
</figure>

## 5. Recuperación

<figure>
  <img src="../../user-guide/images/07-recover.png" alt="Diálogo Recover" width="960">
  <figcaption>El diálogo Recover: destino en otro disco, opciones y los campos opcionales de informe y caso.</figcaption>
</figure>

Seleccione filas y pulse **Recover**:

- **Destino:** un directorio en otro disco. Un destino en el disco de
  origen se rechaza; la anulación para expertos es para quien sabe que
  arriesga los datos que está recuperando. Un destino que sea el propio
  archivo de imagen se rechaza siempre.
- **Opciones:** recrear la estructura de carpetas original, aplicar las
  marcas de tiempo originales, verificar cada archivo con SHA-256.
- **Report and case (optional)** (informe y caso, opcional): elija un archivo de informe
  (`.html`, `.md` o `.json`), opcionalmente calcule el hash de toda la
  fuente para el informe, y rellene número de caso, número de evidencia,
  examinador y notas. Los campos se rellenan previamente con la cabecera de
  adquisición del E01 cuando la fuente tiene una.

La tabla de resultados muestra cada archivo con los bytes escritos,
`verified` o `PARTIAL`, el prefijo del SHA-256 y la ruta de salida. La
ruta del informe se muestra cuando se ha escrito uno.

<figure>
  <img src="../../user-guide/images/08-recovered.png" alt="Tabla de resultados de la recuperación" width="960">
  <figcaption>Tras la recuperación: bytes escritos, estado de verificación y ruta de salida de cada archivo.</figcaption>
</figure>

## Sesiones

Cada escaneo se guarda como sesión (`.phx`) en el directorio de datos de
la aplicación y se lista en la página de inicio. Reabrir una sesión vuelve
a abrir la fuente en el mismo volumen (reparaciones incluidas), de modo
que las previsualizaciones y la recuperación funcionan después sin volver
a escanear. Las sesiones pueden abrirse desde cualquier ruta con
*Browse…*.

## Privacidad

La aplicación no se conecta a la red. Los registros (`-v` en la línea de
comandos, ninguno en la aplicación de escritorio) nunca contienen el
contenido recuperado.
