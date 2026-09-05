# Sí, PHOINIX está «vibecodeado».

Y deliberadamente.

El desarrollo asistido por IA se ha utilizado de forma extensiva para
construir PHOINIX, desde la implementación y las pruebas hasta la
investigación técnica y la revisión de código.

Pero PHOINIX no es el resultado de generar código a ciegas hasta que algo
parece funcionar.

El sistema ha sido **diseñado e ingenierizado deliberadamente**: su
arquitectura de software, sus modelos de recuperación, sus interfaces con
los sistemas de archivos, sus límites de seguridad, su estrategia de
pruebas y su evaluación de la recuperabilidad basada en evidencia se
definieron intencionadamente.

Para una herramienta de recuperación de datos, esa distinción importa.

PHOINIX interactúa con sistemas de archivos dañados, datos borrados,
estructuras de particiones y dispositivos de almacenamiento en bruto. Un
resultado que parece plausible no es suficiente. El comportamiento de la
recuperación debe ser determinista siempre que sea posible, las
suposiciones deben ser explícitas y los resultados deben validarse contra
datos conocidos.

Nuestro principio es simple:

> **Vibecodear la implementación. Ingenierizar el sistema. Verificar el
> resultado.**

La IA nos ayuda a avanzar más rápido. No sustituye a las pruebas, al
criterio técnico, a la revisión por pares ni a la evidencia.

PHOINIX se construye a la vista de todos para que tanto el código como las
decisiones que hay detrás puedan examinarse.

---

Lea la [Declaración de desarrollo](development-declaration.md) completa y
[De dónde viene PHOINIX](origin.md).
