# Aviso legal

PhoinixDR se proporciona «tal cual» y se utiliza enteramente bajo su propio
riesgo. La recuperación de datos es incierta por naturaleza, y un uso
inadecuado puede provocar la pérdida permanente de datos o daños. Siempre
que sea posible, trabaje a partir de una copia o de una imagen de disco y
recupere los archivos en un dispositivo de almacenamiento distinto.

## Qué significa esto en la práctica

- PhoinixDR solo lee las fuentes; nunca escribe en el medio que analiza
  (ADR-0002, ADR-0007). El riesgo está en lo que ocurre alrededor: seguir
  usando el disco que falla mientras se recupera, escribir los archivos
  recuperados en ese mismo disco, o confiar en un archivo recuperado sin
  comprobarlo.
- Trabaje a partir de una imagen de disco cuando el medio esté fallando.
  PhoinixDR abre directamente imágenes RAW, E01, VHD, VHDX y VMDK, así que
  crear la imagen primero no resta ninguna capacidad.
- Recupere en un dispositivo de almacenamiento distinto. El escritor de
  recuperación rechaza un destino situado en el disco de origen; la
  anulación para expertos existe para quien acepta las consecuencias.
- La probabilidad de recuperación y la confianza son estimaciones
  respaldadas por evidencia, no garantías. Lea `phoinix explain` o el panel
  de evidencia antes de confiar en un archivo, y verifique los archivos
  recuperados con los resúmenes SHA-256 que PhoinixDR imprime.
- El software se publica bajo MIT OR Apache-2.0, licencias que excluyen
  toda garantía; véase [LICENSE-MIT](../../LICENSE-MIT) y
  [LICENSE-APACHE](../../LICENSE-APACHE).
