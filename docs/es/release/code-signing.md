# Firma de código y SmartScreen

Un ejecutable sin firmar descargado de internet hace que Windows muestre
«Windows protegió su PC» (Microsoft Defender SmartScreen) la primera vez
que se ejecuta, y algunas heurísticas antivirus señalan a un programa que
lee discos en bruto. Esta página dice qué quita ese aviso, qué no lo
quita, y cómo el flujo de publicación firma los ejecutables en cuanto
existe un certificado.

## Qué es el aviso

SmartScreen es un sistema de reputación, no un veredicto de malware.
Avisa cuando ni el archivo ni su editor tienen suficiente historial de
descargas limpias. Tres hechos deciden qué ayuda:

- **La reputación se gana, no se compra.** La propia guía de Microsoft
  afirma que los certificados de validación extendida (EV) ya no saltan
  SmartScreen: desde el cambio de 2024 en los requisitos del Trusted Root
  Program, todo editor empieza sin reputación y la construye a medida que
  los usuarios descargan y ejecutan versiones firmadas. Pagar un EV solo
  para evitar el aviso no está justificado; el EV sigue importando para
  controladores en modo núcleo y para algunas compras corporativas.
- **La reputación se asocia a la identidad que firma.** Sin firma, la
  reputación se sigue por hash de archivo y cada versión parte de cero.
  Con firma, la identidad del editor acumula reputación entre versiones,
  así que el aviso desaparece tras un periodo que depende del volumen de
  descargas (días o semanas para un proyecto pequeño) y no vuelve para las
  versiones posteriores firmadas con la misma identidad.
- **Verificar no es reputación.** Las sumas SHA-256 de `SHA256SUMS.txt`
  demuestran que una descarga es el archivo que compiló el flujo; no le
  dicen nada a Windows.

Así que la única respuesta honesta a «que SmartScreen deje de avisar» es:
firmar cada versión con el mismo certificado, seguir publicando y dejar
que la reputación se acumule. La Microsoft Store es el único atajo (las
aplicaciones publicadas ahí las vuelve a firmar Microsoft y llevan
reputación completa), a cambio de su empaquetado y su revisión.

## Formas de conseguir un certificado

Desde 2023 el CA/Browser Forum exige que las claves de firma de código
vivan en hardware o en un HSM en la nube, así que un certificado ya no es
un archivo `.pfx` que uno pueda enviarse por correo. Las opciones
prácticas para un proyecto pequeño de código abierto:

| opción | quién puede usarla | coste | notas |
|---|---|---|---|
| **Azure Artifact Signing** (antes Trusted Signing) | organizaciones en la mayoría de países; particulares en EE. UU. y Canadá (identidad validada por Microsoft) | desde 9,99 USD al mes | El servicio del propio Microsoft; la clave nunca sale de Azure; una acción de GitHub firma dentro del flujo; el certificado es de corta duración y se renueva solo, y la reputación sigue a la identidad validada. La mejor opción cuando se puede. |
| **Certificado OV de una CA con firma en la nube** (SSL.com eSigner, DigiCert KeyLocker, Certum SimplySign, GlobalSign, Sectigo) | particulares y organizaciones de todo el mundo, identidad validada por la CA | unos 70 a 400 USD al año | La oferta de Certum para código abierto de particulares es la más barata; la firma se hace con la herramienta en la nube de la CA o con un token USB en su propio equipo, así que el paso del flujo es su CLI o una firma manual de los artefactos descargados seguida de una nueva subida. |
| **SignPath Foundation** | proyectos de código abierto que lo solicitan y son aceptados | gratis | SignPath firma la compilación con su propio certificado a través de su servicio; el flujo envía los artefactos y espera los firmados. La reputación pertenece a la identidad de SignPath Foundation. |
| **Un `.pfx` que ya tenga** | quien tenga un certificado exportable antiguo, o uno autofirmado para pruebas | – | El flujo lo admite; un certificado autofirmado no sirve de nada para SmartScreen. |

## Cómo firma el flujo

El flujo *Release* (`.github/workflows/release.yml`) firma los dos
ejecutables de Windows (`PhoinixDR-<versión>-windows-x64-portable.exe`,
`phoinix-windows-x64.exe`) justo después de compilarlos y comprobarlos,
con resúmenes SHA-256 y un sello de tiempo RFC 3161 para que la firma
sobreviva al certificado. Después verifica cada ejecutable con
`Get-AuthenticodeSignature` y se niega a publicar una versión cuya firma
estaba configurada pero falló. Las notas de la versión dicen si los
ejecutables están firmados y por quién. Sin ninguno de los secretos de
abajo la versión se publica sin firmar, y las notas lo dicen.

### Azure Artifact Signing

1. Cree una cuenta de Artifact Signing en el portal de Azure, complete la
   validación de identidad y cree un perfil de certificado de tipo *Public
   Trust*.
2. Cree un registro de aplicación (entidad de servicio) con un secreto de
   cliente y dele el rol *Trusted Signing Certificate Profile Signer* en la
   cuenta.
3. Añada estos secretos al repositorio (*Settings → Secrets and variables
   → Actions*):

   | secreto | valor |
   |---|---|
   | `AZURE_TENANT_ID` | el ID del directorio (tenant) |
   | `AZURE_CLIENT_ID` | el ID de la aplicación (cliente) |
   | `AZURE_CLIENT_SECRET` | el secreto de cliente |
   | `TRUSTED_SIGNING_ENDPOINT` | el endpoint de la cuenta, `https://<región>.codesigning.azure.net` |
   | `TRUSTED_SIGNING_ACCOUNT` | el nombre de la cuenta |
   | `TRUSTED_SIGNING_PROFILE` | el nombre del perfil de certificado |

4. Lance una publicación. El paso *Sign with Azure Artifact Signing*
   aparece en el trabajo de Windows y las notas dicen «signed by CN=…».

### Un archivo `.pfx`

Codifique el archivo (`[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`)
en el secreto `CODE_SIGNING_PFX_BASE64`, su contraseña en
`CODE_SIGNING_PFX_PASSWORD` y, opcionalmente, un servidor de sellos de
tiempo en `CODE_SIGNING_TIMESTAMP_URL` (por defecto
`http://timestamp.digicert.com`). El flujo escribe el archivo en el
directorio temporal del ejecutor, firma con `signtool` del SDK de Windows
y lo borra.

### Firma en la nube con la herramienta de una CA, o SignPath

Necesitan la CLI de la CA o las acciones de SignPath dentro del flujo;
añádalas al trabajo de Windows entre la comprobación de artefactos y la
comprobación de firmas, para que la verificación y las notas de la
versión las cubran.

## Después de la primera versión firmada

- Mantenga el mismo certificado (o la misma identidad validada, en Azure)
  en todas las versiones: la reputación lo sigue.
- Cuente con que el aviso persista en las primeras versiones; se
  desvanece con las descargas limpias. Nada dentro del ejecutable puede
  acelerarlo.
- Cuando un usuario informe del aviso, indíquele *Más información →
  Ejecutar de todas formas* y el SHA-256 de `SHA256SUMS.txt`, y pídale que
  use el enlace *Notificar que esta aplicación es segura* del aviso: esos
  informes también alimentan la reputación.
