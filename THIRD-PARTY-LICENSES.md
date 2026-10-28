# Licencias de terceros (Third-Party Licenses)

AI Voice-InterConnector se distribuye bajo **MPL-2.0** (ver `LICENSE` y `NOTICE`). El binario
Rust y su archivo de release empaquetan software de terceros bajo sus propias
licencias. Este documento reúne los avisos de copyright y las licencias correspondientes,
cuya preservación exigen dichas licencias al redistribuir el software.

Este inventario se **regenera desde `Cargo.lock`** (lockfile Rust, fuente de verdad del build) y
el metadato `license` de cada crate con `cargo xtask licenses`. La columna «Familia» es una
normalización para agrupar; la columna «Licencia (metadato)» es el dato declarado por
cada crate y prevalece en caso de duda.

> Nota: los pesos de modelos no se empaquetan en el binario; van, vía `setup`, a la raíz de modelos **exclusiva de la aplicación** —`models_cache_dir()` en `crates/avi-shared/src/paths.rs`—: `%LOCALAPPDATA%\ai-voice-interconnector\cache\models` en Windows, `~/Library/Caches/ai-voice-interconnector/models` en macOS y `$XDG_CACHE_HOME/ai-voice-interconnector/models` en Linux. La caché compartida de HuggingFace solo se usa si quien instala define `HF_HUB_CACHE` o `HF_HOME`.

---

## Modelos de voz (no empaquetados)

Los **pesos del modelo** no se empaquetan en el binario: se descargan a la raíz de modelos
exclusiva de la aplicación mediante `ai-voice-interconnector setup`. Se listan por
completitud.

| Modelo | Licencia (verificada en HuggingFace) | Fuente |
|--------|--------------------------------------|--------|
| `qwen3-tts-0.6b` / `qwen3-tts-0.6b-base` (motor Qwen3-TTS, Base opt-in) | **MIT / Apache-2.0** | <https://github.com/QwenLM/Qwen3-TTS> |
| `CristianRojaas/opus-mt-es-en-ct2-int8` / `CristianRojaas/opus-mt-en-es-ct2-int8` (traducción; derivados int8 de Helsinki-NLP/opus-mt) | **CC-BY-4.0** | <https://huggingface.co/CristianRojaas/opus-mt-es-en-ct2-int8>, <https://huggingface.co/CristianRojaas/opus-mt-en-es-ct2-int8>; origen: <https://huggingface.co/Helsinki-NLP/opus-mt-es-en> |
| `istupakov/parakeet-tdt-0.6b-v3-onnx` (Parakeet TDT 0.6B v3 int8, ONNX) | **CC-BY-4.0** | <https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx> |

---

## Familias de licencias permisivas

La mayoría de los crates empaquetados usan licencias permisivas compatibles con
MPL-2.0: **MIT**, **Apache-2.0**, **BSD**, **ISC**, **Unlicense**.

- **MIT** — permiso de uso, copia, modificación y distribución conservando el aviso de
  copyright y la nota de permiso. Texto: <https://opensource.org/license/mit>.
- **Apache-2.0** — conserva avisos de copyright, el texto de la licencia y el archivo
  `NOTICE`. Texto: <https://www.apache.org/licenses/LICENSE-2.0>.
- **BSD (2/3-Clause)** — redistribución conservando el aviso de copyright. Texto:
  <https://opensource.org/license/bsd-3-clause>.
- **ISC** — funcionalmente equivalente a MIT/BSD-2. Texto:
  <https://opensource.org/license/isc-license-txt>.
- **Unlicense** — dominio público. Texto: <https://unlicense.org>.

```
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## Componentes con obligaciones adicionales

Solo crates permisivos y el propio proyecto (MPL-2.0) están presentes en el
binario Rust. El binario Rust no incorpora componentes GPL/AGPL; sus únicas
excepciones son los runtimes del sistema con excepción (GCC Runtime Library
Exception, documentados abajo). El archivo de release añade FFmpeg como
ejecutable independiente, que la aplicación invoca como proceso y no enlaza.
Los pesos de modelos y OpenBLAS (BSD-3-Clause, GCC Runtime Exception) se documentan
en `docs/BUILD.md` §6 (build del motor) y §7 (descarga de modelos).

El archivo de release añade al binario Rust estos componentes binarios de terceros:

- **FFmpeg 9.0.2** (`ffmpeg.exe`/`ffmpeg`): ejecutable independiente de los
  builds GPL de BtbN (Linux), Gyan (Windows) y Martin Riedl (macOS), bajo
  **GPL-3.0-or-later**. La aplicación lo ejecuta como proceso; no forma parte
  del binario Rust ni de su espacio de direcciones. Fuente upstream:
  <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>. Scripts y fuentes de cada
  build: <https://github.com/BtbN/FFmpeg-Builds>,
  <https://www.gyan.dev/ffmpeg/builds/> y <https://ffmpeg.martin-riedl.de/>.
  La oferta de fuentes correspondiente figura en `SOURCE-OFFER.md`.
- **ONNX Runtime** (`onnxruntime.dll`, `libonnxruntime.so`, `libonnxruntime.dylib`):
  la librería oficial de Microsoft, bajo **MIT**
  (<https://github.com/microsoft/onnxruntime/blob/main/LICENSE>).
- **Runtime de Visual C++** (solo Windows: `vcruntime140.dll`, `vcruntime140_1.dll`,
  `msvcp140.dll`, `msvcp140_1.dll` y `vcomp140.dll`): ficheros redistribuibles del Microsoft Visual C++
  Redistributable, que su licencia permite desplegar junto a la aplicación
  (<https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files>).
- **Motor TTS `qwen_tts`**: enlaza estáticamente OpenBLAS (**BSD-3-Clause**) y, en
  Windows y Linux, el runtime de GCC que usa su LAPACK (`libgfortran` y `libgcc` bajo
  GPL-3.0 con la **GCC Runtime Library Exception**, y, en Windows y Linux x86_64,
   `libquadmath` bajo **LGPL-2.1-or-later**, combinable con el proyecto MPL-2.0
   como obra mayor).
- **Runtime de GCC en el binario de Linux**: el binario principal enlaza
  estáticamente `libgomp` (runtime OpenMP de CTranslate2 y oneDNN) y `libstdc++`
  (runtime C++ de CTranslate2, oneDNN, SentencePiece y `cxx`), del gcc-toolset de la imagen de
  build, bajo GPL-3.0 con la **GCC Runtime Library Exception**; `libgcc_s` se
  usa dinámica, del sistema.
- **`alsa-sys` vendorizado** (`vendor/alsa-sys-0.4.0`, solo Linux): copia del
  crate `alsa-sys` 0.4.0 bajo **MIT** (`vendor/alsa-sys-0.4.0/LICENSE`),
  modificada para cargar `libasound.so.2` en tiempo de ejecución con `dlopen` en
  lugar de enlazarla y para comprobar de antemano que define todas las funciones
  que alcanza el binario enlazado (`vendor/alsa-sys-0.4.0/src/required_symbols.rs`,
  generado desde su tabla de símbolos).
- **`ct2rs` vendorizado** (`vendor/ct2rs-0.10.0`): copia del crate `ct2rs`
  0.10.0, con las fuentes de CTranslate2 que incluye, bajo **MIT**
  (`vendor/ct2rs-0.10.0/LICENSE` y `vendor/ct2rs-0.10.0/CTranslate2/LICENSE`),
  modificada para pedir el runtime OpenMP de GNU (`libgomp`) como indica
  `GOMPLIB_<target>` en lugar de siempre como librería dinámica.
- **`onednn-src` vendorizado** (`vendor/onednn-src-0.1.15`): copia del crate
  `onednn-src` 0.1.15 bajo **MIT** (`vendor/onednn-src-0.1.15/LICENSE`), con las
  fuentes de oneDNN que incluye bajo **Apache-2.0**
  (`vendor/onednn-src-0.1.15/oneDNN/LICENSE` y
  `vendor/onednn-src-0.1.15/oneDNN/THIRD-PARTY-PROGRAMS`), modificada para pedir
  la librería estándar de C++ y el runtime OpenMP de GNU que oneDNN usa según
  el contrato `avi-link-runtimes.txt` que escribe un script de CMake propio
  (`vendor/onednn-src-0.1.15/avi-link-runtimes.cmake`), como indican
  `CXXSTDLIB_<target>` y `GOMPLIB_<target>`.

---

## Texto de la licencia GPL-3.0-or-later (FFmpeg)

El ejecutable de FFmpeg se distribuye bajo los términos de la GNU General Public License, versión 3 o posterior. Texto completo:

```
GNU GENERAL PUBLIC LICENSE
                       Version 3, 29 June 2007

 Copyright (C) 2007 Free Software Foundation, Inc. <https://fsf.org/>
 Everyone is permitted to copy and distribute verbatim copies
 of this license document, but changing it is not allowed.

                            Preamble

  The GNU General Public License is a free, copyleft license for
software and other kinds of works.

  The licenses for most software and other practical works are designed
to take away your freedom to share and change the works.  By contrast,
the GNU General Public License is intended to guarantee your freedom to
share and change all versions of a program--to make sure it remains free
software for all its users.  We, the Free Software Foundation, use the
GNU General Public License for most of our software; it applies also to
any other work released this way by its authors.  You can apply it to
your programs, too.

  When we speak of free software, we are referring to freedom, not
price.  Our General Public Licenses are designed to make sure that you
have the freedom to distribute copies of free software (and charge for
them if you wish), that you receive source code or can get it if you
want it, that you can change the software or use pieces of it in new
free programs, and that you know you can do these things.

  To protect your rights, we need to prevent others from denying you
these rights or asking you to surrender the rights.  Therefore, you have
certain responsibilities if you distribute copies of the software, or if
you modify it: responsibilities to respect the freedom of others.

  For example, if you distribute copies of such a program, whether
gratis or for a fee, you must pass on to the recipients the same
freedoms that you received.  You must make sure that they, too, receive
or can get the source code.  And you must show them these terms so they
know their rights.

  Developers that use the GNU GPL protect your rights with two steps:
(1) assert copyright on the software, and (2) offer you this License
giving you legal permission to copy, distribute and/or modify it.

  For the developers' and authors' protection, the GPL clearly explains
that there is no warranty for this free software.  For both users' and
authors' sake, the GPL requires that modified versions be marked as
changed, so that their problems will not be attributed erroneously to
authors of previous versions.

  Some devices are designed to deny users access to install or run
modified versions of the software inside them, although the manufacturer
can do so.  This is fundamentally incompatible with the aim of
protecting users' freedom to change the software.  The systematic
pattern of such abuse occurs in the area of products for individuals to
use, which is precisely where it is most unacceptable.  Therefore, we
have designed this version of the GPL to prohibit the practice for those
products.  If such problems arise substantially in other domains, we
stand ready to extend this provision to those domains in future versions
of the GPL, as needed to protect the freedom of users.

  Finally, every program is threatened constantly by software patents.
States should not allow patents to restrict development and use of
software on general-purpose computers, but in those that do, we wish to
avoid the special danger that patents applied to a free program could
make it effectively proprietary.  To prevent this, the GPL assures that
patents cannot be used to render the program non-free.

  The precise terms and conditions for copying, distribution and
modification follow.

                       TERMS AND CONDITIONS

  0. Definitions.

  "This License" refers to version 3 of the GNU General Public License.

  "Copyright" also means copyright-like laws that apply to other kinds of
works, such as semiconductor masks.

  "The Program" refers to any copyrightable work licensed under this
License.  Each licensee is addressed as "you".  "Licensees" and
"recipients" may be individuals or organizations.

  To "modify" a work means to copy from or adapt all or part of the work
in a fashion requiring copyright permission, other than the making of an
exact copy.  The resulting work is called a "modified version" of the
earlier work or a work "based on" the earlier work.

  A "covered work" means either the unmodified Program or a work based
on the Program.

  To "propagate" a work means to do anything with it that, without
permission, would make you directly or secondarily liable for
infringement under applicable copyright law, except executing it on a
computer or modifying a private copy.  Propagation includes copying,
distribution (with or without modification), making available to the
public, and in some countries other activities as well.

  To "convey" a work means any kind of propagation that enables other
parties to make or receive copies.  Mere interaction with a user through
a computer network, with no transfer of a copy, is not conveying.

  An interactive user interface displays "Appropriate Legal Notices"
to the extent that it includes a convenient and prominently visible
feature that (1) displays an appropriate copyright notice, and (2)
tells the user that there is no warranty for the work (except to the
extent that warranties are provided), that licensees may convey the
work under this License, and how to view a copy of this License.  If
the interface presents a list of user commands or options, such as a
menu, a prominent item in the list meets this criterion.

  1. Source Code.

  The "source code" for a work means the preferred form of the work
for making modifications to it.  "Object code" means any non-source
form of a work.

  A "Standard Interface" means an interface that either is an official
standard defined by a recognized standards body, or, in the case of
interfaces specified for a particular programming language, one that
is widely used among developers working in that language.

  The "System Libraries" of an executable work include anything, other
than the work as a whole, that (a) is included in the normal form of
packaging a Major Component, but which is not part of that Major
Component, and (b) serves only to enable use of the work with that
Major Component, or to implement a Standard Interface for which an
implementation is available to the public in source code form.  A
"Major Component", in this context, means a major essential component
(kernel, window system, and so on) of the specific operating system
(if any) on which the executable work runs, or a compiler used to
produce the work, or an object code interpreter used to run it.

  The "Corresponding Source" for a work in object code form means all
the source code needed to generate, install, and (for an executable
work) run the object code and to modify the work, including scripts to
control those activities.  However, it does not include the work's
System Libraries, or general-purpose tools or generally available free
programs which are used unmodified in performing those activities but
which are not part of the work.  For example, Corresponding Source
includes interface definition files associated with source files for
the work, and the source code for shared libraries and dynamically
linked subprograms that the work is specifically designed to require,
such as by intimate data communication or control flow between those
subprograms and other parts of the work.

  The Corresponding Source need not include anything that users
can regenerate automatically from other parts of the Corresponding
Source.

  The Corresponding Source for a work in source code form is that
same work.

  2. Basic Permissions.

  All rights granted under this License are granted for the term of
copyright on the Program, and are irrevocable provided the stated
conditions are met.  This License explicitly affirms your unlimited
permission to run the unmodified Program.  The output from running a
covered work is covered by this License only if the output, given its
content, constitutes a covered work.  This License acknowledges your
rights of fair use or other equivalent, as provided by copyright law.

  You may make, run and propagate covered works that you do not
convey, without conditions so long as your license otherwise remains
in force.  You may convey covered works to others for the sole purpose
of having them make modifications exclusively for you, or provide you
with facilities for running those works, provided that you comply with
the terms of this License in conveying all material for which you do
not control copyright.  Those thus making or running the covered works
for you must do so exclusively on your behalf, under your direction
and control, on terms that prohibit them from making any copies of
your copyrighted material outside their relationship with you.

  Conveying under any other circumstances is permitted solely under
the conditions stated below.  Sublicensing is not allowed; section 10
makes it unnecessary.

  3. Protecting Users' Legal Rights From Anti-Circumvention Law.

  No covered work shall be deemed part of an effective technological
measure under any applicable law fulfilling obligations under article
11 of the WIPO copyright treaty adopted on 20 December 1996, or
similar laws prohibiting or restricting circumvention of such
measures.

  When you convey a covered work, you waive any legal power to forbid
circumvention of technological measures to the extent such circumvention
is effected by exercising rights under this License with respect to
the covered work, and you disclaim any intention to limit operation or
modification of the work as a means of enforcing, against the work's
users, your or third parties' legal rights to forbid circumvention of
technological measures.

  4. Conveying Verbatim Copies.

  You may convey verbatim copies of the Program's source code as you
receive it, in any medium, provided that you conspicuously and
appropriately publish on each copy an appropriate copyright notice;
keep intact all notices stating that this License and any
non-permissive terms added in accord with section 7 apply to the code;
keep intact all notices of the absence of any warranty; and give all
recipients a copy of this License along with the Program.

  You may charge any price or no price for each copy that you convey,
and you may offer support or warranty protection for a fee.

  5. Conveying Modified Source Versions.

  You may convey a work based on the Program, or the modifications to
produce it from the Program, in the form of source code under the
terms of section 4, provided that you also meet all of these conditions:

    a) The work must carry prominent notices stating that you modified
    it, and giving a relevant date.

    b) The work must carry prominent notices stating that it is
    released under this License and any conditions added under section
    7.  This requirement modifies the requirement in section 4 to
    "keep intact all notices".

    c) You must license the entire work, as a whole, under this
    License to anyone who comes into possession of a copy.  This
    License will therefore apply, along with any applicable section 7
    additional terms, to the whole of the work, and all its parts,
    regardless of how they are packaged.  This License gives no
    permission to license the work in any other way, but it does not
    invalidate such permission if you have separately received it.

    d) If the work has interactive user interfaces, each must display
    Appropriate Legal Notices; however, if the Program has interactive
    interfaces that do not display Appropriate Legal Notices, your
    work need not make them do so.

  A compilation of a covered work with other separate and independent
works, which are not by their nature extensions of the covered work,
and which are not combined with it such as to form a larger program,
in or on a volume of a storage or distribution medium, is called an
"aggregate" if the compilation and its resulting copyright are not
used to limit the access or legal rights of the compilation's users
beyond what the individual works permit.  Inclusion of a covered work
in an aggregate does not cause this License to apply to the other
parts of the aggregate.

  6. Conveying Non-Source Forms.

  You may convey a covered work in object code form under the terms
of sections 4 and 5, provided that you also convey the
machine-readable Corresponding Source under the terms of this License,
in one of these ways:

    a) Convey the object code in, or embodied in, a physical product
    (including a physical distribution medium), accompanied by the
    Corresponding Source fixed on a durable physical medium
    customarily used for software interchange.

    b) Convey the object code in, or embodied in, a physical product
    (including a physical distribution medium), accompanied by a
    written offer, valid for at least three years and valid for as
    long as you offer spare parts or customer support for that product
    model, to give anyone who possesses the object code either (1) a
    copy of the Corresponding Source for all the software in the
    product that is covered by this License, on a durable physical
    medium customarily used for software interchange, for a price no
    more than your reasonable cost of physically performing this
    conveying of source, or (2) access to copy the
    Corresponding Source from a network server at no charge.

    c) Convey individual copies of the object code with a copy of the
    written offer to provide the Corresponding Source.  This
    alternative is allowed only occasionally and noncommercially, and
    only if you received the object code with such an offer, in accord
    with subsection 6b.

    d) Convey the object code by offering access from a designated
    place (gratis or for a charge), and offer equivalent access to the
    Corresponding Source in the same way through the same place at no
    further charge.  You need not require recipients to copy the
    Corresponding Source along with the object code.  If the place to
    copy the object code is a network server, the Corresponding Source
    may be on a different server (operated by you or a third party)
    that supports equivalent copying facilities, provided you maintain
    clear directions next to the object code saying where to find the
    Corresponding Source.  Regardless of what server hosts the
    Corresponding Source, you remain obligated to ensure that it is
    available for as long as needed to satisfy these requirements.

    e) Convey the object code using peer-to-peer transmission, provided
    you inform other peers where the object code and Corresponding
    Source of the work are being offered to the general public at no
    charge under subsection 6d.

  A separable portion of the object code, whose source code is excluded
from the Corresponding Source as a System Library, need not be
included in conveying the object code work.

  A "User Product" is either (1) a "consumer product", which means any
tangible personal property which is normally used for personal, family,
or household purposes, or (2) anything designed or sold for incorporation
into a dwelling.  In determining whether a product is a consumer product,
doubtful cases shall be resolved in favor of coverage.  For a particular
product received by a particular user, "normally used" refers to a
typical or common use of that class of product, regardless of the status
of the particular user or of the way in which the particular user
actually uses, or expects or is expected to use, the product.  A product
is a consumer product regardless of whether the product has substantial
commercial, industrial or non-consumer uses, unless such uses represent
the only significant mode of use of the product.

  "Installation Information" for a User Product means any methods,
procedures, authorization keys, or other information required to install
and execute modified versions of a covered work in that User Product from
a modified version of its Corresponding Source.  The information must
suffice to ensure that the continued functioning of the modified object
code is in no case prevented or interfered with solely because
modification has been made.

  If you convey an object code work under this section in, or with, or
specifically for use in, a User Product, and the conveying occurs as
part of a transaction in which the right of possession and use of the
User Product is transferred to the recipient in perpetuity or for a
fixed term (regardless of how the transaction is characterized), the
Corresponding Source conveyed under this section must be accompanied
by the Installation Information.  But this requirement does not apply
if neither you nor any third party retains the ability to install
modified object code on the User Product (for example, the work has
been installed in ROM).

  The requirement to provide Installation Information does not include a
requirement to continue to provide support service, warranty, or updates
for a work that has been modified or installed by the recipient, or for
the User Product in which it has been modified or installed.  Access to a
network may be denied when the modification itself materially and
adversely affects the operation of the network or violates the rules and
protocols for communication across the network.

  Corresponding Source conveyed, and Installation Information provided,
in accord with this section must be in a format that is publicly
documented (and with an implementation available to the public in
source code form), and must require no special password or key for
unpacking, reading or copying.

  7. Additional Terms.

  "Additional permissions" are terms that supplement the terms of this
License by making exceptions from one or more of its conditions.
Additional permissions that are applicable to the entire Program shall
be treated as though they were included in this License, to the extent
that they are valid under applicable law.  If additional permissions
apply only to part of the Program, that part may be used separately
under those permissions, but the entire Program remains governed by
this License without regard to the additional permissions.

  When you convey a copy of a covered work, you may at your option
remove any additional permissions from that copy, or from any part of
it.  (Additional permissions may be written to require their own
removal in certain cases when you modify the work.)  You may place
additional permissions on material, added by you to a covered work,
for which you have or can give appropriate copyright permission.

  Notwithstanding any other provision of this License, for material you
add to a covered work, you may (if authorized by the copyright holders of
that material) supplement the terms of this License with terms:

    a) Disclaiming warranty or limiting liability differently from the
    terms of sections 15 and 16 of this License; or

    b) Requiring preservation of specified reasonable legal notices or
    author attributions in that material or in the Appropriate Legal
    Notices displayed by works containing it; or

    c) Prohibiting misrepresentation of the origin of that material, or
    requiring that modified versions of such material be marked in
    reasonable ways as different from the original version; or

    d) Limiting the use for publicity purposes of names of licensors or
    authors of the material; or

    e) Declining to grant rights under trademark law for use of some
    trade names, trademarks, or service marks; or

    f) Requiring indemnification of licensors and authors of that
    material by anyone who conveys the material (or modified versions of
    it) with contractual assumptions of liability to the recipient, for
    any liability that these contractual assumptions directly impose on
    those licensors and authors.

  All other non-permissive additional terms are considered "further
restrictions" within the meaning of section 10.  If the Program as you
received it, or any part of it, contains a notice stating that it is
governed by this License along with a term that is a further
restriction, you may remove that term.  If a license document contains
a further restriction but permits relicensing or conveying under this
License, you may add to a covered work material governed by the terms
of that license document, provided that the further restriction does
not survive such relicensing or conveying.

  If you add terms to a covered work in accord with this section, you
must place, in the relevant source files, a statement of the
additional terms that apply to those files, or a notice indicating
where to find the applicable terms.

  Additional terms, permissive or non-permissive, may be stated in the
form of a separately written license, or stated as exceptions;
the above requirements apply either way.

  8. Termination.

  You may not propagate or modify a covered work except as expressly
provided under this License.  Any attempt otherwise to propagate or
modify it is void, and will automatically terminate your rights under
this License (including any patent licenses granted under the third
paragraph of section 11).

  However, if you cease all violation of this License, then your
license from a particular copyright holder is reinstated (a)
provisionally, unless and until the copyright holder explicitly and
finally terminates your license, and (b) permanently, if the copyright
holder fails to notify you of the violation by some reasonable means
prior to 60 days after the cessation.

  Moreover, your license from a particular copyright holder is
reinstated permanently if the copyright holder notifies you of the
violation by some reasonable means, this is the first time you have
received notice of violation of this License (for any work) from that
copyright holder, and you cure the violation prior to 30 days after
your receipt of the notice.

  Termination of your rights under this section does not terminate the
licenses of parties who have received copies or rights from you under
this License.  If your rights have been terminated and not permanently
reinstated, you do not qualify to receive new licenses for the same
material under section 10.

  9. Acceptance Not Required for Having Copies.

  You are not required to accept this License in order to receive or
run a copy of the Program.  Ancillary propagation of a covered work
occurring solely as a consequence of using peer-to-peer transmission
to receive a copy likewise does not require acceptance.  However,
nothing other than this License grants you permission to propagate or
modify any covered work.  These actions infringe copyright if you do
not accept this License.  Therefore, by modifying or propagating a
covered work, you indicate your acceptance of this License to do so.

  10. Automatic Licensing of Downstream Recipients.

  Each time you convey a covered work, the recipient automatically
receives a license from the original licensors, to run, modify and
propagate that work, subject to this License.  You are not responsible
for enforcing compliance by third parties with this License.

  An "entity transaction" is a transaction transferring control of an
organization, or substantially all assets of one, or subdividing an
organization, or merging organizations.  If propagation of a covered
work results from an entity transaction, each party to that
transaction who receives a copy of the work also receives whatever
licenses to the work the party's predecessor in interest had or could
give under the previous paragraph, plus a right to possession of the
Corresponding Source of the work from the predecessor in interest, if
the predecessor has it or can get it with reasonable efforts.

  You may not impose any further restrictions on the exercise of the
rights granted or affirmed under this License.  For example, you may
not impose a license fee, royalty, or other charge for exercise of
rights granted under this License, and you may not initiate litigation
(including a cross-claim or counterclaim in a lawsuit) alleging that
any patent claim is infringed by making, using, selling, offering for
sale, or importing the Program or any portion of it.

  11. Patents.

  A "contributor" is a copyright holder who authorizes use under this
License of the Program or a work on which the Program is based.  The
work thus licensed is called the contributor's "contributor version".

  A contributor's "essential patent claims" are all patent claims
owned or controlled by the contributor, whether already acquired or
hereafter acquired, that would be infringed by some manner, permitted
by this License, of making, using, or selling its contributor version,
but do not include claims that would be infringed only as a
consequence of further modification of the contributor version.  For
purposes of this definition, "control" includes the right to grant
patent sublicenses in a manner consistent with the requirements of
this License.

  Each contributor grants you a non-exclusive, worldwide, royalty-free
patent license under the contributor's essential patent claims, to
make, use, sell, offer for sale, import and otherwise run, modify and
propagate the contents of its contributor version.

  In the following three paragraphs, a "patent license" is any express
agreement or commitment, however denominated, not to enforce a patent
(such as an express permission to practice a patent or covenant not to
sue for patent infringement).  To "grant" such a patent license to a
party means to make such an agreement or commitment not to enforce a
patent against the party.

  If you convey a covered work, knowingly relying on a patent license,
and the Corresponding Source of the work is not available for anyone
to copy, free of charge and under the terms of this License, through a
publicly available network server or other readily accessible means,
then you must either (1) cause the Corresponding Source to be so
available, or (2) arrange to deprive yourself of the benefit of the
patent license for this particular work, or (3) arrange, in a manner
consistent with the requirements of this License, to extend the patent
license to downstream recipients.  "Knowingly relying" means you have
actual knowledge that, but for the patent license, your conveying the
covered work in a country, or your recipient's use of the covered work
in a country, would infringe one or more identifiable patents in that
country that you have reason to believe are valid.

  If, pursuant to or in connection with a single transaction or
arrangement, you convey, or propagate by procuring conveyance of, a
covered work, and grant a patent license to some of the parties
receiving the covered work authorizing them to use, propagate, modify
or convey a specific copy of the covered work, then the patent license
you grant is automatically extended to all recipients of the covered
work and works based on it.

  A patent license is "discriminatory" if it does not include within
the scope of its coverage, prohibits the exercise of, or is
conditioned on the non-exercise of one or more of the rights that are
specifically granted under this License.  You may not convey a covered
work if you are a party to an arrangement with a third party that is
in the business of distributing software, under which you make payment
to the third party based on the extent of your activity of conveying
the work, and under which the third party grants, to any of the
parties who would receive the covered work from you, a discriminatory
patent license (a) in connection with copies of the covered work
conveyed by you (or copies made from those copies), or (b) primarily
for and in connection with specific products or compilations that
contain the covered work, unless you entered into that arrangement,
or that patent license was granted, prior to 28 March 2007.

  Nothing in this License shall be construed as excluding or limiting
any implied license or other defenses to infringement that may
otherwise be available to you under applicable patent law.

  12. No Surrender of Others' Freedom.

  If conditions are imposed on you (whether by court order, agreement or
otherwise) that contradict the conditions of this License, they do not
excuse you from the conditions of this License.  If you cannot convey a
covered work so as to satisfy simultaneously your obligations under this
License and any other pertinent obligations, then as a consequence you may
not convey it at all.  For example, if you agree to terms that obligate you
to collect a royalty for further conveying from those to whom you convey
the Program, the only way you could satisfy both those terms and this
License would be to refrain entirely from conveying the Program.

  13. Use with the GNU Affero General Public License.

  Notwithstanding any other provision of this License, you have
permission to link or combine any covered work with a work licensed
under version 3 of the GNU Affero General Public License into a single
combined work, and to convey the resulting work.  The terms of this
License will continue to apply to the part which is the covered work,
but the special requirements of the GNU Affero General Public License,
section 13, concerning interaction through a network will apply to the
combination as such.

  14. Revised Versions of this License.

  The Free Software Foundation may publish revised and/or new versions of
the GNU General Public License from time to time.  Such new versions will
be similar in spirit to the present version, but may differ in detail to
address new problems or concerns.

  Each version is given a distinguishing version number.  If the
Program specifies that a certain numbered version of the GNU General
Public License "or any later version" applies to it, you have the
option of following the terms and conditions either of that numbered
version or of any later version published by the Free Software
Foundation.  If the Program does not specify a version number of the
GNU General Public License, you may choose any version ever published
by the Free Software Foundation.

  If the Program specifies that a proxy can decide which future
versions of the GNU General Public License can be used, that proxy's
public statement of acceptance of a version permanently authorizes you
to choose that version for the Program.

  Later license versions may give you additional or different
permissions.  However, no additional obligations are imposed on any
author or copyright holder as a result of your choosing to follow a
later version.

  15. Disclaimer of Warranty.

  THERE IS NO WARRANTY FOR THE PROGRAM, TO THE EXTENT PERMITTED BY
APPLICABLE LAW.  EXCEPT WHEN OTHERWISE STATED IN WRITING THE COPYRIGHT
HOLDERS AND/OR OTHER PARTIES PROVIDE THE PROGRAM "AS IS" WITHOUT WARRANTY
OF ANY KIND, EITHER EXPRESSED OR IMPLIED, INCLUDING, BUT NOT LIMITED TO,
THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
PURPOSE.  THE ENTIRE RISK AS TO THE QUALITY AND PERFORMANCE OF THE PROGRAM
IS WITH YOU.  SHOULD THE PROGRAM PROVE DEFECTIVE, YOU ASSUME THE COST OF
ALL NECESSARY SERVICING, REPAIR OR CORRECTION.

  16. Limitation of Liability.

  IN NO EVENT UNLESS REQUIRED BY APPLICABLE LAW OR AGREED TO IN WRITING
WILL ANY COPYRIGHT HOLDER, OR ANY OTHER PARTY WHO MODIFIES AND/OR CONVEYS
THE PROGRAM AS PERMITTED ABOVE, BE LIABLE TO YOU FOR DAMAGES, INCLUDING ANY
GENERAL, SPECIAL, INCIDENTAL OR CONSEQUENTIAL DAMAGES ARISING OUT OF THE
USE OR INABILITY TO USE THE PROGRAM (INCLUDING BUT NOT LIMITED TO LOSS OF
DATA OR DATA BEING RENDERED INACCURATE OR LOSSES SUSTAINED BY YOU OR THIRD
PARTIES OR A FAILURE OF THE PROGRAM TO OPERATE WITH ANY OTHER PROGRAMS),
EVEN IF SUCH HOLDER OR OTHER PARTY HAS BEEN ADVISED OF THE POSSIBILITY OF
SUCH DAMAGES.

  17. Interpretation of Sections 15 and 16.

  If the disclaimer of warranty and limitation of liability provided
above cannot be given local legal effect according to their terms,
reviewing courts shall apply local law that most closely approximates
an absolute waiver of all civil liability in connection with the
Program, unless a warranty or assumption of liability accompanies a
copy of the Program in return for a fee.

                     END OF TERMS AND CONDITIONS

            How to Apply These Terms to Your New Programs

  If you develop a new program, and you want it to be of the greatest
possible use to the public, the best way to achieve this is to make it
free software which everyone can redistribute and change under these terms.

  To do so, attach the following notices to the program.  It is safest
to attach them to the start of each source file to most effectively
state the exclusion of warranty; and each file should have at least
the "copyright" line and a pointer to where the full notice is found.

    <one line to give the program's name and a brief idea of what it does.>
    Copyright (C) <year>  <name of author>

    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with this program.  If not, see <https://www.gnu.org/licenses/>.

Also add information on how to contact you by electronic and paper mail.

  If the program does terminal interaction, make it output a short
notice like this when it starts in an interactive mode:

    <program>  Copyright (C) <year>  <name of author>
    This program comes with ABSOLUTELY NO WARRANTY; for details type `show w'.
    This is free software, and you are welcome to redistribute it
    under certain conditions; type `show c' for details.

The hypothetical commands `show w' and `show c' should show the appropriate
parts of the General Public License.  Of course, your program's commands
might be different; for a GUI interface, you would use an "about box".

  You should also get your employer (if you work as a programmer) or school,
if any, to sign a "copyright disclaimer" for the program, if necessary.
For more information on this, and how to apply and follow the GNU GPL, see
<https://www.gnu.org/licenses/>.

  The GNU General Public License does not permit incorporating your program
into proprietary programs.  If your program is a subroutine library, you
may consider it more useful to permit linking proprietary applications with
the library.  If this is what you want to do, use the GNU Lesser General
Public License instead of this License.  But first, please read
<https://www.gnu.org/licenses/why-not-lgpl.html>.
```

## Inventario completo del lockfile

<!-- inventario:inicio -->
Generado desde `Cargo.lock` (510 paquetes resueltos, 469 crates únicos, directos y transitivos).
Resumen por familia (paquetes resueltos): Apache-2.0 22, BSD 3, CDLA-Permissive-2.0 2, ISC 3, MIT 441, MPL-2.0 15, Unicode-3.0 18, Zlib 6.

| Paquete | Versión | Licencia (metadato) | Familia |
|---------|---------|---------------------|--------|
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | MIT |
| `ahash` | 0.8.12 | MIT OR Apache-2.0 | MIT |
| `aho-corasick` | 1.1.5 | Unlicense OR MIT | MIT |
| `ai-voice-interconnector` | 0.37.0 | MPL-2.0 | MPL-2.0 |
| `alsa` | 0.11.0 | Apache-2.0/MIT | MIT |
| `alsa-sys` | 0.4.0 | MIT | MIT |
| `android_system_properties` | 0.1.6 | MIT OR Apache-2.0 | MIT |
| `anstream` | 1.0.0 | MIT OR Apache-2.0 | MIT |
| `anstyle` | 1.0.14 | MIT OR Apache-2.0 | MIT |
| `anstyle-parse` | 1.0.0 | MIT OR Apache-2.0 | MIT |
| `anstyle-query` | 1.1.5 | MIT OR Apache-2.0 | MIT |
| `anstyle-wincon` | 3.0.11 | MIT OR Apache-2.0 | MIT |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | MIT |
| `approx` | 0.5.1 | Apache-2.0 | Apache-2.0 |
| `arbitrary` | 1.4.2 | MIT OR Apache-2.0 | MIT |
| `arrayvec` | 0.7.8 | MIT OR Apache-2.0 | MIT |
| `async-trait` | 0.1.92 | MIT OR Apache-2.0 | MIT |
| `atomic-waker` | 1.1.2 | Apache-2.0 OR MIT | MIT |
| `audio_thread_priority` | 0.35.1 | MPL-2.0 | MPL-2.0 |
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT | MIT |
| `avi-audio` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-core` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-daemon` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-lifecycle` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-process` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-shared` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-store` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-stt` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-translation` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `avi-tts` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `aws-lc-rs` | 1.18.0 | ISC AND (Apache-2.0 OR ISC) | Apache-2.0 |
| `aws-lc-sys` | 0.44.0 | ISC AND (Apache-2.0 OR ISC) AND Apache-2.0 AND MIT AND BSD-3-Clause AND (Apache-2.0 OR ISC OR MIT) AND (Apache-2.0 OR ISC OR MIT-0) | MIT |
| `axum` | 0.7.9 | MIT | MIT |
| `axum-core` | 0.4.5 | MIT | MIT |
| `base64` | 0.13.1 | MIT/Apache-2.0 | MIT |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | MIT |
| `bitflags` | 2.13.1 | MIT OR Apache-2.0 | MIT |
| `blake3` | 1.8.7 | CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception | Apache-2.0 |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 | MIT |
| `block-buffer` | 0.12.1 | MIT OR Apache-2.0 | MIT |
| `block2` | 0.6.2 | MIT | MIT |
| `bon` | 3.10.0 | MIT OR Apache-2.0 | MIT |
| `bon-macros` | 3.10.0 | MIT OR Apache-2.0 | MIT |
| `bstr` | 1.13.1 | MIT OR Apache-2.0 | MIT |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 | MIT |
| `bytemuck` | 1.25.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `byteorder` | 1.5.0 | Unlicense OR MIT | MIT |
| `bytes` | 1.12.1 | MIT | MIT |
| `castaway` | 0.2.4 | MIT | MIT |
| `cc` | 1.4.2 | MIT OR Apache-2.0 | MIT |
| `cfg-if` | 0.1.10 | MIT/Apache-2.0 | MIT |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 | MIT |
| `cfg_aliases` | 0.2.2 | MIT | MIT |
| `chacha20` | 0.10.1 | MIT OR Apache-2.0 | MIT |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 | MIT |
| `clap` | 4.6.6 | MIT OR Apache-2.0 | MIT |
| `clap_builder` | 4.6.6 | MIT OR Apache-2.0 | MIT |
| `clap_derive` | 4.6.4 | MIT OR Apache-2.0 | MIT |
| `clap_lex` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `cmake` | 0.1.58 | MIT OR Apache-2.0 | MIT |
| `codespan-reporting` | 0.13.1 | Apache-2.0 | Apache-2.0 |
| `colorchoice` | 1.0.5 | MIT OR Apache-2.0 | MIT |
| `colored` | 3.1.1 | MPL-2.0 | MPL-2.0 |
| `combine` | 4.6.7 | MIT | MIT |
| `compact_str` | 0.9.1 | MIT | MIT |
| `console` | 0.15.11 | MIT | MIT |
| `console` | 0.16.4 | MIT | MIT |
| `const-oid` | 0.10.2 | Apache-2.0 OR MIT | MIT |
| `const-str` | 1.1.0 | MIT | MIT |
| `const_panic` | 0.2.17 | Zlib | Zlib |
| `constant_time_eq` | 0.4.2 | CC0-1.0 OR MIT-0 OR Apache-2.0 | MIT |
| `core-foundation` | 0.9.4 | MIT OR Apache-2.0 | MIT |
| `core-foundation` | 0.10.1 | MIT OR Apache-2.0 | MIT |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 | MIT |
| `coreaudio-rs` | 0.14.2 | MIT/Apache-2.0 | MIT |
| `countio` | 0.3.0 | MIT | MIT |
| `cpal` | 0.18.2 | Apache-2.0 | Apache-2.0 |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 | MIT |
| `cpufeatures` | 0.3.0 | MIT OR Apache-2.0 | MIT |
| `crc` | 3.4.0 | MIT OR Apache-2.0 | MIT |
| `crc-catalog` | 2.5.0 | MIT OR Apache-2.0 | MIT |
| `crc32fast` | 1.5.1 | MIT OR Apache-2.0 | MIT |
| `crossbeam-channel` | 0.5.16 | MIT OR Apache-2.0 | MIT |
| `crossbeam-deque` | 0.8.7 | MIT OR Apache-2.0 | MIT |
| `crossbeam-epoch` | 0.9.20 | MIT OR Apache-2.0 | MIT |
| `crossbeam-utils` | 0.8.22 | MIT OR Apache-2.0 | MIT |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 | MIT |
| `crypto-common` | 0.2.2 | MIT OR Apache-2.0 | MIT |
| `ct2rs` | 0.10.0 | MIT | MIT |
| `ctor` | 1.0.13 | Apache-2.0 OR MIT | MIT |
| `ctrlc` | 3.5.2 | MIT/Apache-2.0 | MIT |
| `cxx` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxx-build` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxxbridge-cmd` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxxbridge-flags` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `cxxbridge-macro` | 1.0.199 | MIT OR Apache-2.0 | MIT |
| `darling` | 0.20.11 | MIT | MIT |
| `darling` | 0.24.1 | MIT | MIT |
| `darling_core` | 0.20.11 | MIT | MIT |
| `darling_core` | 0.24.1 | MIT | MIT |
| `darling_macro` | 0.20.11 | MIT | MIT |
| `darling_macro` | 0.24.1 | MIT | MIT |
| `dary_heap` | 0.3.9 | MIT OR Apache-2.0 | MIT |
| `dasp_sample` | 0.11.0 | MIT OR Apache-2.0 | MIT |
| `denoise` | 1.0.0 | MIT | MIT |
| `deranged` | 0.5.8 | MIT OR Apache-2.0 | MIT |
| `derive_arbitrary` | 1.4.2 | MIT OR Apache-2.0 | MIT |
| `derive_builder` | 0.20.2 | MIT OR Apache-2.0 | MIT |
| `derive_builder_core` | 0.20.2 | MIT OR Apache-2.0 | MIT |
| `derive_builder_macro` | 0.20.2 | MIT OR Apache-2.0 | MIT |
| `digest` | 0.10.7 | MIT OR Apache-2.0 | MIT |
| `digest` | 0.11.3 | MIT OR Apache-2.0 | MIT |
| `directories` | 5.0.1 | MIT OR Apache-2.0 | MIT |
| `dirs` | 6.0.0 | MIT OR Apache-2.0 | MIT |
| `dirs-sys` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `dirs-sys` | 0.5.0 | MIT OR Apache-2.0 | MIT |
| `dispatch2` | 0.3.1 | Zlib OR Apache-2.0 OR MIT | MIT |
| `displaydoc` | 0.2.7 | MIT OR Apache-2.0 | MIT |
| `dunce` | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | MIT |
| `either` | 1.17.0 | MIT OR Apache-2.0 | MIT |
| `encode_unicode` | 1.0.0 | Apache-2.0 OR MIT | MIT |
| `encoding_rs` | 0.8.35 | (Apache-2.0 OR MIT) AND BSD-3-Clause | MIT |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT | MIT |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | MIT |
| `esaxx-rs` | 0.1.10 | Apache-2.0 | Apache-2.0 |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT | MIT |
| `filetime` | 0.2.29 | MIT/Apache-2.0 | MIT |
| `find-msvc-tools` | 0.1.10 | MIT OR Apache-2.0 | MIT |
| `flate2` | 1.1.10 | MIT OR Apache-2.0 | MIT |
| `fnv` | 1.0.7 | Apache-2.0 / MIT | MIT |
| `foldhash` | 0.2.0 | Zlib | Zlib |
| `form_urlencoded` | 1.2.2 | MIT OR Apache-2.0 | MIT |
| `fs_extra` | 1.3.0 | MIT | MIT |
| `futures` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-channel` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-core` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-executor` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-io` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-macro` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-sink` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-task` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `futures-util` | 0.3.34 | MIT OR Apache-2.0 | MIT |
| `gearhash` | 0.1.3 | MIT OR Apache-2.0 | MIT |
| `generic-array` | 0.14.7 | MIT | MIT |
| `getrandom` | 0.2.17 | MIT OR Apache-2.0 | MIT |
| `getrandom` | 0.3.4 | MIT OR Apache-2.0 | MIT |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | MIT |
| `git-version` | 0.3.9 | BSD-2-Clause | BSD |
| `git-version-macro` | 0.3.9 | BSD-2-Clause | BSD |
| `globset` | 0.4.20 | Unlicense OR MIT | MIT |
| `h2` | 0.4.19 | MIT | MIT |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 | MIT |
| `heapify` | 0.2.0 | MIT OR Apache-2.0 | MIT |
| `heck` | 0.5.0 | MIT OR Apache-2.0 | MIT |
| `hermit-abi` | 0.5.2 | MIT OR Apache-2.0 | MIT |
| `hf-hub` | 1.0.0 | Apache-2.0 | Apache-2.0 |
| `hf-xet` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `home` | 0.5.12 | MIT OR Apache-2.0 | MIT |
| `hound` | 3.5.1 | Apache-2.0 | Apache-2.0 |
| `http` | 1.5.0 | MIT OR Apache-2.0 | MIT |
| `http-body` | 1.1.0 | MIT | MIT |
| `http-body-util` | 0.1.4 | MIT | MIT |
| `httparse` | 1.10.1 | MIT OR Apache-2.0 | MIT |
| `httpdate` | 1.0.3 | MIT OR Apache-2.0 | MIT |
| `humantime` | 2.4.0 | MIT OR Apache-2.0 | MIT |
| `hybrid-array` | 0.4.14 | MIT OR Apache-2.0 | MIT |
| `hyper` | 1.11.0 | MIT | MIT |
| `hyper-rustls` | 0.27.9 | Apache-2.0 OR ISC OR MIT | MIT |
| `hyper-util` | 0.1.20 | MIT | MIT |
| `iana-time-zone` | 0.1.65 | MIT OR Apache-2.0 | MIT |
| `iana-time-zone-haiku` | 0.1.2 | MIT OR Apache-2.0 | MIT |
| `icu_collections` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_locale_core` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_normalizer` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_normalizer_data` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_properties` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_properties_data` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `icu_provider` | 2.3.0 | Unicode-3.0 | Unicode-3.0 |
| `ident_case` | 1.0.1 | MIT/Apache-2.0 | MIT |
| `idna` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `idna_adapter` | 1.2.2 | Apache-2.0 OR MIT | MIT |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT | MIT |
| `indicatif` | 0.17.11 | MIT | MIT |
| `indicatif` | 0.18.6 | MIT | MIT |
| `ipnet` | 2.12.1 | MIT OR Apache-2.0 | MIT |
| `is_terminal_polyfill` | 1.70.2 | MIT OR Apache-2.0 | MIT |
| `itertools` | 0.14.0 | MIT OR Apache-2.0 | MIT |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | MIT |
| `jni` | 0.22.4 | MIT OR Apache-2.0 | MIT |
| `jni-macros` | 0.22.4 | MIT OR Apache-2.0 | MIT |
| `jni-sys` | 0.3.1 | MIT OR Apache-2.0 | MIT |
| `jni-sys` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `jni-sys-macros` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `jobserver` | 0.1.35 | MIT OR Apache-2.0 | MIT |
| `js-sys` | 0.3.104 | MIT OR Apache-2.0 | MIT |
| `konst` | 0.4.3 | Zlib | Zlib |
| `konst_proc_macros` | 0.4.1 | Zlib | Zlib |
| `lazy_static` | 1.5.0 | MIT OR Apache-2.0 | MIT |
| `libc` | 0.2.189 | MIT OR Apache-2.0 | MIT |
| `libloading` | 0.9.0 | ISC | ISC |
| `libredox` | 0.1.19 | MIT | MIT |
| `link-cplusplus` | 1.0.12 | MIT OR Apache-2.0 | MIT |
| `link-section` | 0.19.3 | Apache-2.0 OR MIT | MIT |
| `linktime-proc-macro` | 0.2.3 | Apache-2.0 OR MIT | MIT |
| `linux-raw-sys` | 0.4.15 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `litemap` | 0.8.3 | Unicode-3.0 | Unicode-3.0 |
| `lock_api` | 0.4.14 | MIT OR Apache-2.0 | MIT |
| `log` | 0.4.33 | MIT OR Apache-2.0 | MIT |
| `lru-slab` | 0.1.2 | MIT OR Apache-2.0 OR Zlib | MIT |
| `lz4_flex` | 0.13.1 | MIT | MIT |
| `lzma-rs` | 0.3.0 | MIT | MIT |
| `mach2` | 0.4.3 | BSD-2-Clause OR MIT OR Apache-2.0 | MIT |
| `mach2` | 0.6.0 | BSD-2-Clause OR MIT OR Apache-2.0 | MIT |
| `macro_rules_attribute` | 0.2.3 | Apache-2.0 OR MIT OR Zlib | MIT |
| `macro_rules_attribute-proc_macro` | 0.2.3 | Apache-2.0 OR MIT OR Zlib | MIT |
| `matchers` | 0.2.0 | MIT | MIT |
| `matchit` | 0.7.3 | MIT AND BSD-3-Clause | MIT |
| `matrixmultiply` | 0.3.11 | MIT/Apache-2.0 | MIT |
| `memchr` | 2.8.3 | Unlicense OR MIT | MIT |
| `mime` | 0.3.17 | MIT OR Apache-2.0 | MIT |
| `mime_guess` | 2.0.5 | MIT | MIT |
| `minimal-lexical` | 0.2.1 | MIT/Apache-2.0 | MIT |
| `miniz_oxide` | 0.9.1 | MIT OR Zlib OR Apache-2.0 | MIT |
| `mio` | 1.2.2 | MIT | MIT |
| `monostate` | 0.1.18 | MIT OR Apache-2.0 | MIT |
| `monostate-impl` | 0.1.18 | MIT OR Apache-2.0 | MIT |
| `more-asserts` | 0.3.1 | Unlicense OR MIT OR Apache-2.0 OR CC0-1.0 | MIT |
| `ndarray` | 0.17.2 | MIT OR Apache-2.0 | MIT |
| `ndk` | 0.9.0 | MIT OR Apache-2.0 | MIT |
| `ndk-context` | 0.1.1 | MIT OR Apache-2.0 | MIT |
| `ndk-sys` | 0.6.0+11769913 | MIT OR Apache-2.0 | MIT |
| `nix` | 0.31.3 | MIT | MIT |
| `nom` | 7.1.3 | MIT | MIT |
| `ntapi` | 0.4.3 | Apache-2.0 OR MIT | MIT |
| `nu-ansi-term` | 0.50.3 | MIT | MIT |
| `num-complex` | 0.4.6 | MIT OR Apache-2.0 | MIT |
| `num-conv` | 0.2.2 | MIT OR Apache-2.0 | MIT |
| `num-derive` | 0.4.2 | MIT OR Apache-2.0 | MIT |
| `num-integer` | 0.1.46 | MIT OR Apache-2.0 | MIT |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 | MIT |
| `num_cpus` | 1.17.0 | MIT OR Apache-2.0 | MIT |
| `num_enum` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | MIT |
| `num_enum_derive` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | MIT |
| `number_prefix` | 0.4.0 | MIT | MIT |
| `objc2` | 0.6.4 | MIT | MIT |
| `objc2-audio-toolbox` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-avf-audio` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-core-audio` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-core-audio-types` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-core-foundation` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-encode` | 4.1.0 | MIT | MIT |
| `objc2-foundation` | 0.3.2 | MIT | MIT |
| `objc2-io-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `objc2-system-configuration` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | MIT |
| `object` | 0.40.0 | Apache-2.0 OR MIT | MIT |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 | MIT |
| `once_cell_polyfill` | 1.70.2 | MIT OR Apache-2.0 | MIT |
| `onednn-src` | 0.1.15 | MIT | MIT |
| `oneshot` | 0.1.13 | MIT OR Apache-2.0 | MIT |
| `onig` | 6.5.3 | MIT | MIT |
| `onig_sys` | 69.9.3 | MIT | MIT |
| `openssl-probe` | 0.2.1 | MIT OR Apache-2.0 | MIT |
| `option-ext` | 0.2.0 | MPL-2.0 | MPL-2.0 |
| `ort` | 2.0.0-rc.13 | MIT OR Apache-2.0 | MIT |
| `ort-sys` | 2.0.0-rc.13 | MIT OR Apache-2.0 | MIT |
| `os_str_bytes` | 6.6.1 | MIT OR Apache-2.0 | MIT |
| `parking_lot` | 0.12.5 | MIT OR Apache-2.0 | MIT |
| `parking_lot_core` | 0.9.12 | MIT OR Apache-2.0 | MIT |
| `paste` | 1.0.15 | MIT OR Apache-2.0 | MIT |
| `pastey` | 0.2.3 | MIT OR Apache-2.0 | MIT |
| `pathdiff` | 0.2.3 | MIT/Apache-2.0 | MIT |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 | MIT |
| `pin-project` | 1.1.13 | Apache-2.0 OR MIT | MIT |
| `pin-project-internal` | 1.1.13 | Apache-2.0 OR MIT | MIT |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT | MIT |
| `pkg-config` | 0.3.33 | MIT OR Apache-2.0 | MIT |
| `portable-atomic` | 1.15.0 | Apache-2.0 OR MIT | MIT |
| `portable-atomic-util` | 0.2.7 | Apache-2.0 OR MIT | MIT |
| `potential_utf` | 0.1.6 | Unicode-3.0 | Unicode-3.0 |
| `powerfmt` | 0.2.0 | MIT OR Apache-2.0 | MIT |
| `ppv-lite86` | 0.2.21 | MIT OR Apache-2.0 | MIT |
| `prettyplease` | 0.3.0 | MIT OR Apache-2.0 | MIT |
| `primal-check` | 0.3.4 | MIT OR Apache-2.0 | MIT |
| `proc-macro-crate` | 3.5.0 | MIT OR Apache-2.0 | MIT |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | MIT |
| `prost` | 0.14.4 | Apache-2.0 | Apache-2.0 |
| `prost-derive` | 0.14.4 | Apache-2.0 | Apache-2.0 |
| `quinn` | 0.11.11 | MIT OR Apache-2.0 | MIT |
| `quinn-proto` | 0.11.17 | MIT OR Apache-2.0 | MIT |
| `quinn-udp` | 0.5.15 | MIT OR Apache-2.0 | MIT |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | MIT |
| `r-efi` | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | MIT |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | MIT |
| `rand` | 0.9.5 | MIT OR Apache-2.0 | MIT |
| `rand` | 0.10.2 | MIT OR Apache-2.0 | MIT |
| `rand_chacha` | 0.9.0 | MIT OR Apache-2.0 | MIT |
| `rand_core` | 0.9.5 | MIT OR Apache-2.0 | MIT |
| `rand_core` | 0.10.1 | MIT OR Apache-2.0 | MIT |
| `rand_pcg` | 0.10.2 | MIT OR Apache-2.0 | MIT |
| `rawpointer` | 0.2.1 | MIT/Apache-2.0 | MIT |
| `rayon` | 1.12.0 | MIT OR Apache-2.0 | MIT |
| `rayon-cond` | 0.4.0 | Apache-2.0/MIT | MIT |
| `rayon-core` | 1.13.0 | MIT OR Apache-2.0 | MIT |
| `realfft` | 3.5.0 | MIT | MIT |
| `redb` | 3.1.3 | MIT OR Apache-2.0 | MIT |
| `redox_syscall` | 0.5.18 | MIT | MIT |
| `redox_users` | 0.4.6 | MIT | MIT |
| `redox_users` | 0.5.2 | MIT | MIT |
| `regex` | 1.13.1 | MIT OR Apache-2.0 | MIT |
| `regex-automata` | 0.4.18 | MIT OR Apache-2.0 | MIT |
| `regex-syntax` | 0.8.11 | MIT OR Apache-2.0 | MIT |
| `reqwest` | 0.12.28 | MIT OR Apache-2.0 | MIT |
| `reqwest` | 0.13.4 | MIT OR Apache-2.0 | MIT |
| `reqwest-middleware` | 0.5.2 | MIT OR Apache-2.0 | MIT |
| `ring` | 0.17.14 | Apache-2.0 AND ISC | Apache-2.0 |
| `rubato` | 0.16.2 | MIT | MIT |
| `rustc-hash` | 2.1.3 | Apache-2.0 OR MIT | MIT |
| `rustc_version` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `rustfft` | 6.4.1 | MIT OR Apache-2.0 | MIT |
| `rustix` | 0.38.44 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `rustix` | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `rustls` | 0.23.46 | Apache-2.0 OR ISC OR MIT | MIT |
| `rustls-native-certs` | 0.8.4 | Apache-2.0 OR ISC OR MIT | MIT |
| `rustls-pki-types` | 1.15.1 | MIT OR Apache-2.0 | MIT |
| `rustls-platform-verifier` | 0.7.0 | MIT OR Apache-2.0 | MIT |
| `rustls-platform-verifier-android` | 0.1.1 | MIT OR Apache-2.0 | MIT |
| `rustls-webpki` | 0.103.15 | ISC | ISC |
| `rustversion` | 1.0.23 | MIT OR Apache-2.0 | MIT |
| `ryu` | 1.0.23 | Apache-2.0 OR BSL-1.0 | Apache-2.0 |
| `safe-transmute` | 0.11.3 | MIT | MIT |
| `same-file` | 1.0.6 | Unlicense/MIT | MIT |
| `schannel` | 0.1.29 | MIT | MIT |
| `scopeguard` | 1.2.0 | MIT OR Apache-2.0 | MIT |
| `scratch` | 1.0.9 | MIT OR Apache-2.0 | MIT |
| `security-framework` | 3.7.0 | MIT OR Apache-2.0 | MIT |
| `security-framework-sys` | 2.17.0 | MIT OR Apache-2.0 | MIT |
| `semver` | 1.0.28 | MIT OR Apache-2.0 | MIT |
| `sentencepiece` | 0.13.2 | MIT OR Apache-2.0 | MIT |
| `sentencepiece-sys` | 0.13.2 | Apache-2.0 | Apache-2.0 |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | MIT |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | MIT |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | MIT |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 | MIT |
| `serde_path_to_error` | 0.1.20 | MIT OR Apache-2.0 | MIT |
| `serde_repr` | 0.1.21 | MIT OR Apache-2.0 | MIT |
| `serde_urlencoded` | 0.7.1 | MIT/Apache-2.0 | MIT |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | MIT |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | MIT |
| `sharded-slab` | 0.1.7 | MIT | MIT |
| `shellexpand` | 3.1.2 | MIT/Apache-2.0 | MIT |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | MIT |
| `signal-hook-registry` | 1.4.8 | MIT OR Apache-2.0 | MIT |
| `simd-adler32` | 0.3.10 | MIT | MIT |
| `simd_cesu8` | 1.2.0 | Apache-2.0 OR MIT | MIT |
| `simdutf8` | 0.1.5 | MIT OR Apache-2.0 | MIT |
| `slab` | 0.4.12 | MIT | MIT |
| `smallvec` | 1.15.2 | MIT OR Apache-2.0 | MIT |
| `socket2` | 0.6.5 | MIT OR Apache-2.0 | MIT |
| `spm_precompiled` | 0.1.4 | Apache-2.0 | Apache-2.0 |
| `stable_deref_trait` | 1.2.1 | MIT OR Apache-2.0 | MIT |
| `static_assertions` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `statrs` | 0.18.0 | MIT | MIT |
| `strength_reduce` | 0.2.4 | MIT OR Apache-2.0 | MIT |
| `strsim` | 0.11.1 | MIT | MIT |
| `subtle` | 2.6.1 | BSD-3-Clause | BSD |
| `symlink` | 0.1.0 | MIT/Apache-2.0 | MIT |
| `syn` | 2.0.119 | MIT OR Apache-2.0 | MIT |
| `syn` | 3.0.3 | MIT OR Apache-2.0 | MIT |
| `sync_wrapper` | 1.0.2 | Apache-2.0 | Apache-2.0 |
| `synstructure` | 0.13.2 | MIT | MIT |
| `sysinfo` | 0.38.4 | MIT | MIT |
| `system-configuration` | 0.7.0 | MIT OR Apache-2.0 | MIT |
| `system-configuration-sys` | 0.6.0 | MIT OR Apache-2.0 | MIT |
| `tar` | 0.4.46 | MIT OR Apache-2.0 | MIT |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | MIT |
| `termcolor` | 1.4.1 | Unlicense OR MIT | MIT |
| `thiserror` | 1.0.69 | MIT OR Apache-2.0 | MIT |
| `thiserror` | 2.0.20 | MIT OR Apache-2.0 | MIT |
| `thiserror-impl` | 1.0.69 | MIT OR Apache-2.0 | MIT |
| `thiserror-impl` | 2.0.20 | MIT OR Apache-2.0 | MIT |
| `thread_local` | 1.1.10 | MIT OR Apache-2.0 | MIT |
| `time` | 0.3.55 | MIT OR Apache-2.0 | MIT |
| `time-core` | 0.1.9 | MIT OR Apache-2.0 | MIT |
| `time-macros` | 0.2.32 | MIT OR Apache-2.0 | MIT |
| `tinystr` | 0.8.4 | Unicode-3.0 | Unicode-3.0 |
| `tinyvec` | 1.12.0 | Zlib OR Apache-2.0 OR MIT | MIT |
| `tinyvec_macros` | 0.1.1 | MIT OR Apache-2.0 OR Zlib | MIT |
| `tokenizers` | 0.22.2 | Apache-2.0 | Apache-2.0 |
| `tokio` | 1.53.1 | MIT | MIT |
| `tokio-macros` | 2.7.2 | MIT | MIT |
| `tokio-retry` | 0.3.2 | MIT | MIT |
| `tokio-rustls` | 0.26.4 | MIT OR Apache-2.0 | MIT |
| `tokio-stream` | 0.1.19 | MIT | MIT |
| `tokio-util` | 0.7.19 | MIT | MIT |
| `tokio_with_wasm` | 0.8.8 | MIT | MIT |
| `tokio_with_wasm_proc` | 0.8.8 | MIT | MIT |
| `toml_datetime` | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | MIT |
| `toml_edit` | 0.25.13+spec-1.1.0 | MIT OR Apache-2.0 | MIT |
| `toml_parser` | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | MIT |
| `tower` | 0.5.3 | MIT | MIT |
| `tower-http` | 0.6.11 | MIT | MIT |
| `tower-layer` | 0.3.3 | MIT | MIT |
| `tower-service` | 0.3.3 | MIT | MIT |
| `tracing` | 0.1.44 | MIT | MIT |
| `tracing-appender` | 0.2.5 | MIT | MIT |
| `tracing-attributes` | 0.1.31 | MIT | MIT |
| `tracing-core` | 0.1.36 | MIT | MIT |
| `tracing-log` | 0.2.0 | MIT | MIT |
| `tracing-serde` | 0.2.0 | MIT | MIT |
| `tracing-subscriber` | 0.3.23 | MIT | MIT |
| `transpose` | 0.2.3 | MIT OR Apache-2.0 | MIT |
| `try-lock` | 0.2.5 | MIT | MIT |
| `twox-hash` | 2.1.3 | MIT | MIT |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | MIT |
| `typewit` | 1.15.2 | Zlib | Zlib |
| `unicase` | 2.9.0 | MIT OR Apache-2.0 | MIT |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 | MIT |
| `unicode-normalization-alignments` | 0.1.12 | MIT/Apache-2.0 | MIT |
| `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 | MIT |
| `unicode-width` | 0.2.2 | MIT OR Apache-2.0 | MIT |
| `unicode_categories` | 0.1.1 | MIT OR Apache-2.0 | MIT |
| `unit-prefix` | 0.5.2 | MIT | MIT |
| `untrusted` | 0.9.0 | ISC | ISC |
| `url` | 2.5.8 | MIT OR Apache-2.0 | MIT |
| `urlencoding` | 2.1.3 | MIT | MIT |
| `utf8_iter` | 1.0.4 | Apache-2.0 OR MIT | MIT |
| `utf8parse` | 0.2.2 | Apache-2.0 OR MIT | MIT |
| `uuid` | 1.25.0 | Apache-2.0 OR MIT | MIT |
| `valuable` | 0.1.1 | MIT | MIT |
| `version_check` | 0.9.5 | MIT/Apache-2.0 | MIT |
| `walkdir` | 2.5.0 | Unlicense/MIT | MIT |
| `want` | 0.3.1 | MIT | MIT |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `wasi` | 0.14.7+wasi-0.2.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `wasip2` | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `wasite` | 1.0.2 | Apache-2.0 OR BSL-1.0 OR MIT | MIT |
| `wasm-bindgen` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-futures` | 0.4.77 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-macro` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-macro-support` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-bindgen-shared` | 0.2.127 | MIT OR Apache-2.0 | MIT |
| `wasm-streams` | 0.4.2 | MIT OR Apache-2.0 | MIT |
| `wasm-streams` | 0.5.0 | MIT OR Apache-2.0 | MIT |
| `web-sys` | 0.3.104 | MIT OR Apache-2.0 | MIT |
| `web-time` | 1.1.0 | MIT OR Apache-2.0 | MIT |
| `webpki-root-certs` | 1.0.9 | CDLA-Permissive-2.0 | CDLA-Permissive-2.0 |
| `webpki-roots` | 1.0.11 | CDLA-Permissive-2.0 | CDLA-Permissive-2.0 |
| `which` | 4.4.2 | MIT | MIT |
| `whoami` | 2.1.3 | Apache-2.0 OR BSL-1.0 OR MIT | MIT |
| `winapi` | 0.3.9 | MIT/Apache-2.0 | MIT |
| `winapi-i686-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | MIT |
| `winapi-util` | 0.1.11 | Unlicense OR MIT | MIT |
| `winapi-x86_64-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | MIT |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | MIT |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 | MIT |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | MIT |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | MIT |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 | MIT |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 | MIT |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | MIT |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 | MIT |
| `windows-registry` | 0.6.1 | MIT OR Apache-2.0 | MIT |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 | MIT |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 | MIT |
| `windows-sys` | 0.48.0 | MIT OR Apache-2.0 | MIT |
| `windows-sys` | 0.52.0 | MIT OR Apache-2.0 | MIT |
| `windows-sys` | 0.59.0 | MIT OR Apache-2.0 | MIT |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | MIT |
| `windows-targets` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows-targets` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 | MIT |
| `windows_aarch64_gnullvm` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows_aarch64_gnullvm` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_aarch64_msvc` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows_aarch64_msvc` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_i686_gnu` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows_i686_gnu` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_i686_gnullvm` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_i686_msvc` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows_i686_msvc` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_gnu` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_gnu` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_gnullvm` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_gnullvm` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_msvc` | 0.48.5 | MIT OR Apache-2.0 | MIT |
| `windows_x86_64_msvc` | 0.52.6 | MIT OR Apache-2.0 | MIT |
| `winnow` | 1.0.4 | MIT | MIT |
| `wit-bindgen` | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | MIT |
| `writeable` | 0.6.4 | Unicode-3.0 | Unicode-3.0 |
| `xattr` | 1.6.1 | MIT OR Apache-2.0 | MIT |
| `xet-client` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xet-core-structures` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xet-data` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xet-runtime` | 1.6.0 | Apache-2.0 | Apache-2.0 |
| `xtask` | 0.1.0 | MPL-2.0 | MPL-2.0 |
| `yoke` | 0.8.3 | Unicode-3.0 | Unicode-3.0 |
| `yoke-derive` | 0.8.2 | Unicode-3.0 | Unicode-3.0 |
| `zerocopy` | 0.8.56 | BSD-2-Clause OR Apache-2.0 OR MIT | MIT |
| `zerocopy-derive` | 0.8.56 | BSD-2-Clause OR Apache-2.0 OR MIT | MIT |
| `zerofrom` | 0.1.8 | Unicode-3.0 | Unicode-3.0 |
| `zerofrom-derive` | 0.1.7 | Unicode-3.0 | Unicode-3.0 |
| `zeroize` | 1.9.0 | Apache-2.0 OR MIT | MIT |
| `zerotrie` | 0.2.5 | Unicode-3.0 | Unicode-3.0 |
| `zerovec` | 0.11.7 | Unicode-3.0 | Unicode-3.0 |
| `zerovec-derive` | 0.11.4 | Unicode-3.0 | Unicode-3.0 |
| `zip` | 2.4.2 | MIT | MIT |
| `zlib-rs` | 0.6.7 | Zlib | Zlib |
| `zmij` | 1.0.23 | MIT | MIT |
| `zopfli` | 0.8.3 | Apache-2.0 | Apache-2.0 |
<!-- inventario:fin -->

> `ai-voice-interconnector`, `avi-*` y `xtask` son crates locales del proyecto (MPL-2.0).

---

## Regeneración

Este inventario se regenera de forma **deliberada** tras actualizar `Cargo.lock`:

```bash
cargo xtask licenses          # reescribe la región del inventario
cargo xtask licenses --check  # verifica sincronía completa
```

El generador reescribe solo la región entre los marcadores `<!-- inventario:inicio -->` y
`<!-- inventario:fin -->`: una fila por versión resuelta en `Cargo.lock`, con la licencia
de `cargo metadata --all-features`, y el conteo y el resumen por familia calculados. Falla
ante una expresión de licencia sin familia asignada o un paquete sin licencia declarada.
`--check` compara la región completa (nombre, versión, licencia, familia y totales).

Revisar el diff resultante para auditar altas/bajas de dependencias y cambios de licencia
antes de commitear.