# De dónde viene PHOINIX

PHOINIX nació de una observación sencilla: las herramientas de
recuperación de datos de código abierto son potentes, pero no siempre son
fáciles de usar.

Proyectos como TestDisk y PhotoRec llevan años demostrando que la
recuperación de datos seria puede hacerse con software de código abierto.
Admiten una amplia gama de sistemas de archivos, estructuras de
almacenamiento, técnicas de recuperación de particiones y métodos de
tallado de archivos. También son referencias inestimables para entender lo
que una herramienta de recuperación capaz debería poder hacer.

Pero PHOINIX nunca pretendió ser una utilidad de recuperación más, y nunca
pretendió ser una interfaz gráfica colocada encima de TestDisk.

El objetivo desde el principio fue más amplio: crear una plataforma de
recuperación de datos completa y moderna que combine un motor de
recuperación potente con una interfaz gráfica intuitiva, accesible y útil
tanto para usuarios corrientes como para profesionales técnicos.

Queríamos que un usuario pudiera seleccionar un disco, iniciar un escaneo,
entender lo que se ha encontrado, ver la probabilidad de que un archivo
sea recuperable, previsualizarlo cuando sea posible y recuperarlo sin
tener que entender antes las interioridades de los sistemas de archivos,
las tablas de particiones, las estructuras de inodos, los registros de la
MFT ni las técnicas de tallado de archivos.

Al mismo tiempo, no queríamos que la sencillez de la interfaz significara
sencillez en el motor.

Por eso PHOINIX está diseñado como un sistema de recuperación completo:
el acceso a dispositivos en bruto, el análisis de particiones, la
recuperación de borrados consciente del sistema de archivos, el tallado de
archivos, la recuperación de particiones perdidas, la validación
estructural de archivos, la evaluación de la recuperabilidad, el soporte
de imágenes de disco, las herramientas de línea de comandos y la
aplicación gráfica de escritorio forman parte de la misma arquitectura.

Otra decisión importante se tomó al principio del proyecto: **PHOINIX no se
construiría copiando ni incorporando código fuente de proyectos de
recuperación de código abierto existentes.**

Los proyectos existentes se tratan como estado del arte, referencias
técnicas, puntos de comparación y fuentes de requisitos. Estudiamos qué
admiten, cómo abordan los problemas de recuperación, qué estructuras de
los sistemas de archivos importan, qué esperan los usuarios de una
herramienta de recuperación madura y dónde las soluciones existentes son
fuertes o limitadas.

Sus funciones ayudan a definir el espacio del problema. Su documentación,
su comportamiento publicado, las referencias de los sistemas de archivos,
los estándares y el conocimiento técnico público nos ayudan a entenderlo.

Pero PHOINIX implementa su propio motor de recuperación, sus estructuras
de datos, sus interfaces, su modelo de puntuación, su flujo de trabajo y su
experiencia de usuario.

Cuando se reutilizan intencionadamente bibliotecas de terceros maduras, se
integran mediante adaptadores explícitos y aislados y bajo sus respectivas
licencias. No definen la arquitectura interna de PHOINIX.

Esta distinción importa porque el objetivo no es reproducir TestDisk,
PhotoRec, Recuva ni ninguna otra aplicación existente.

El objetivo es construir algo que aprenda de todas ellas siguiendo una
filosofía de diseño diferente:

> **Lo bastante potente para una recuperación seria. Lo bastante simple
> para que cualquiera lo use. Lo bastante abierto para que cualquiera lo
> inspeccione, lo mejore y confíe en él.**

PHOINIX existe porque creemos que la recuperación de datos de código
abierto puede ser a la vez técnicamente sofisticada y realmente fácil de
usar, y que los usuarios no deberían tener que elegir entre las dos cosas.

---

Véase también la [Declaración de desarrollo](development-declaration.md) y
[Sí, PHOINIX está «vibecodeado»](vibecoded.md).
