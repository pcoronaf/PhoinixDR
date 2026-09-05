# Where PHOINIX Came From

PHOINIX started from a simple observation: open-source data recovery tools
are powerful, but they are not always easy to use.

Projects such as TestDisk and PhotoRec have demonstrated for years that
serious data recovery can be done with open-source software. They support a
wide range of filesystems, storage structures, partition-recovery
techniques, and file-carving methods. They are also invaluable references
for understanding what a capable recovery tool should be able to do.

But PHOINIX was never intended to be just another recovery utility, and it
was never intended to be a graphical interface placed on top of TestDisk.

The goal from the beginning was broader: to create a complete, modern
data-recovery platform that combines a powerful recovery engine with a
graphical interface that is intuitive, approachable, and useful to both
ordinary users and technical professionals.

We wanted a user to be able to select a disk, start a scan, understand what
was found, see how likely a file is to be recoverable, preview it when
possible, and recover it without first having to understand filesystem
internals, partition tables, inode structures, MFT records, or file-carving
techniques.

At the same time, we did not want simplicity in the interface to mean
simplicity in the engine.

PHOINIX is therefore designed as a complete recovery system: raw device
access, partition analysis, filesystem-aware undelete, file carving,
lost-partition recovery, structural file validation, recoverability
assessment, disk-image support, command-line tools, and a graphical desktop
application are all parts of the same architecture.

Another important decision was made early in the project: **PHOINIX would
not be built by copying or incorporating source code from existing
open-source recovery projects.**

Existing projects are treated as prior art, technical references,
benchmarks, and sources of requirements. We study what they support, how
recovery problems are approached, which filesystem structures matter, what
users expect from mature recovery tools, and where existing solutions are
strong or limited.

Their features help define the problem space. Their documentation, published
behavior, filesystem references, standards, and public technical knowledge
help us understand it.

But PHOINIX implements its own recovery engine, data structures, interfaces,
scoring model, workflow, and user experience.

Where mature third-party libraries are intentionally reused, they are
integrated through explicit, isolated adapters and under their respective
licenses. They do not define PHOINIX's internal architecture.

This distinction matters because the objective is not to reproduce
TestDisk, PhotoRec, Recuva, or any other existing application.

The objective is to build something that learns from all of them while
following a different design philosophy:

> **Powerful enough for serious recovery. Simple enough for anyone to use.
> Open enough for anyone to inspect, improve, and trust.**

PHOINIX exists because we believe open-source data recovery can be both
technically sophisticated and genuinely user-friendly—and that users should
not have to choose between the two.

---

See also the [Development Declaration](development-declaration.md) and
[Yes, PHOINIX is vibecoded](vibecoded.md).
