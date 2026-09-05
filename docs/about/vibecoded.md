# Yes, PHOINIX is vibecoded.

And deliberately so.

AI-assisted development has been used extensively to build PHOINIX, from
implementation and testing to technical research and code review.

But PHOINIX is not the result of blindly generating code until something
appears to work.

The system has been **deliberately designed and engineered**: its software
architecture, recovery models, filesystem interfaces, safety boundaries,
test strategy, and evidence-based recovery assessment were defined
intentionally.

For a data-recovery tool, that distinction matters.

PHOINIX interacts with damaged filesystems, deleted data, partition
structures, and raw storage devices. A plausible-looking result is not
enough. Recovery behavior must be deterministic where possible, assumptions
must be explicit, and results must be validated against known data.

Our principle is simple:

> **Vibecode the implementation. Engineer the system. Verify the result.**

AI helps us move faster. It does not replace testing, technical judgment,
peer review, or evidence.

PHOINIX is built in the open so that both the code and the decisions behind
it can be examined.

---

Read the full [Development Declaration](development-declaration.md) and
[Where PHOINIX Came From](origin.md).
