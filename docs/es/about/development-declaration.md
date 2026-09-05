# Declaración de desarrollo

PHOINIX se desarrolló con un uso extensivo de programación asistida por
IA, lo que a menudo se describe como *vibecoding*.

Eso no significa que el proyecto se creara simplemente pidiendo a una IA
que generara una aplicación y aceptando el resultado.

PHOINIX ha sido diseñado deliberadamente como un sistema de software. Su
arquitectura, los límites entre componentes, los modelos de datos, las
estrategias de recuperación, las abstracciones de los sistemas de
archivos, los requisitos de seguridad, la metodología de pruebas, el
modelo de licencia y la hoja de ruta de desarrollo se definieron
intencionadamente antes y durante la implementación.

Las herramientas de IA se utilizan para acelerar la implementación,
explorar alternativas, generar y revisar código, crear pruebas, analizar
documentación técnica y ayudar en la depuración. Las decisiones técnicas,
sin embargo, se evalúan frente a los requisitos del proyecto y no se
aceptan solo porque las haya sugerido o generado un sistema de IA.

Se presta especial atención a las áreas en las que los errores pueden
tener consecuencias graves, entre ellas:

- el acceso de solo lectura a los medios de origen;
- el análisis de sistemas de archivos y particiones;
- las comprobaciones de límites y de seguridad de enteros;
- la evaluación de la recuperación y de la sobrescritura;
- el tratamiento de estructuras de almacenamiento malformadas o corruptas;
- las pruebas de recuperación deterministas;
- las pruebas de fuzzing;
- la validación contra imágenes de disco conocidas y hashes
  criptográficos;
- la separación clara entre evidencia observada, inferencia e
  incertidumbre.

Por tanto, PHOINIX adopta el vibecoding como acelerador del desarrollo,
pero no como sustituto de la ingeniería de software.

Una descripción más precisa del proyecto es:

> **Asistido por IA, diseñado deliberadamente, ingenierizado
> sistemáticamente y verificado continuamente.**

Se espera que toda capacidad de recuperación importante sea explicable,
comprobable, reproducible y revisable.

El hecho de que la IA haya contribuido a la implementación nunca debe
considerarse una prueba de que el software es correcto. La corrección debe
provenir de la disciplina de ingeniería, las pruebas, la revisión
independiente y la validación empírica.

PHOINIX es de código abierto, en parte, para que estas suposiciones,
algoritmos, decisiones de diseño e implementaciones puedan ser
inspeccionados y cuestionados por otros.

## Cómo se refleja esto en el repositorio

| principio | dónde verlo |
|---|---|
| acceso de solo lectura a los medios de origen | `BlockReader` no tiene primitiva de escritura ([ADR-0002](../../decisions/ADR-0002-read-only-blockreader.md), [ADR-0007](../../decisions/ADR-0007-no-source-writes-in-recovery-core.md)); el escritor de recuperación rechaza destinos en el disco de origen |
| límites y seguridad de enteros | `unsafe_code = "deny"` en todos los crates de recuperación; clippy prohíbe la indexación directa, los `unwrap` y las conversiones con pérdida fuera de las pruebas; aritmética comprobada en `phoinix-core::arith` |
| estructuras malformadas | cada analizador se ejercita con rondas de corrupción que alteran bytes y truncan imágenes (`tests/integration`) |
| pruebas de recuperación deterministas | los fixtures se generan con scripts a partir de contenido conocido con resúmenes SHA-256 registrados (`tests/generated`, `tests/fixtures/*/manifest.json`) |
| validación contra imágenes y hashes conocidos | se verifican los hashes almacenados en los E01, se calcula el hash de cada archivo recuperado y cada recuperación de fixture se compara byte a byte |
| evidencia, inferencia e incertidumbre | la *probabilidad* de recuperación y la *confianza* de la evaluación son números separados con razones impresas ([modelo de salud](../../recovery/health-model.md), [ADR-0006](../../decisions/ADR-0006-likelihood-vs-confidence.md)) |
| decisiones de diseño | los [registros de decisiones de arquitectura](../../decisions/README.md) |
