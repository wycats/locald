---
title: "Generative Design & The Agent Lexicon"
---

This document defines the meta-framework we use to build `locald`. It is not just about _what_ we build, but _how_ we think about the system to ensure it remains coherent, navigable, and scalable for both humans and AI agents.

> **Optional background**: This is design context and not required for the core workflow.

## 1. The Core Philosophy: Generative vs. Descriptive

We strive for **Generative** systems over **Descriptive** ones.

- **Descriptive (The Laundry List)**: A collection of independent rules or features. Requires rote memorization.
  - _Example_: "Do X. Don't do Y. Do Z."
  - _Metaphor_: **The Bouquet**. Cut flowers arranged beautifully. Static. Cannot grow.
- **Generative (The Seed)**: A set of root principles from which the rules naturally emerge.
  - _Example_: "Follow Principle A." (From which X, Y, and Z are derived).
  - _Metaphor_: **The Seed**. Contains the DNA to grow the plant. Dynamic. Adapts to context.

**Goal**: When defining a new feature or axiom, ask: _"Is this a cut flower, or is it a seed?"_

## 2. The Tool: "Name and Frame"

To communicate complex generative concepts efficiently, we use the **Name and Frame** pattern.

- **Name (The Engineer)**: The standard technical term, pattern, or principle. Gives us precision and searchability.
- **Frame (The Philosopher)**: A cultural, biological, or physical metaphor. Gives us the _intuition_ and _nuance_ of the concept.

### Example: The Laundry List Problem

- **Name**: **Conceptual Integrity** (Fred Brooks). The system must reflect a single set of design ideas.
- **Frame**: **Generative Grammar** (Linguistics). Don't give me a dictionary of valid sentences; give me the grammar rules to generate them.

## 3. The Medium: Optimizing for Agents

While "Generative" describes the _quality_ of the ideas (Content), we also need strict rules for how those ideas are _stored and accessed_ (Medium) to overcome the limitations of an AI Agent (limited context, no long-term memory).

### A. Is it Canon? (Persistence)

- **Name**: **Single Source of Truth**.
- **Frame**: **Canon vs. Apocrypha**.
  - **Apocrypha**: The Chat. Ephemeral, interesting, but vanishes when the window closes.
  - **Canon**: The Workspace (`AGENTS.md`, `docs/`, Code). The enduring truth that survives the session.
- **Rule**: If it's not in the Canon, it doesn't exist.

### B. Is it Rooted? (Reachability)

- **Name**: **Traversal / Reachability**.
- **Frame**: **The Golden Thread**.
  - _Intuition_: A fresh agent is like Theseus in the Labyrinth. It has access to every file, but without a link from a known context (The Root), it is lost.
- **Rule**: Information must be reachable via a deterministic path from the root (`AGENTS.md` or `README.md`). Orphan nodes are invisible.

### C. Is it Distilled? (Density)

- **Name**: **Lossless Compression**.
- **Frame**: **The Reduction** (Culinary).
  - _Intuition_: Boiling down a gallon of stock (context) into a spoon of demi-glace (axiom).
- **Rule**: Remove redundancy without losing meaning. A reduction only works if you start with good stock.

## 4. The Emergent Result: Deterministic Discovery

When a system is both **Generative** (Predictable Content) and **Rooted** (Traversable Structure), it creates a powerful emergent property.

**The Formula**: `Generative Content + Rooted Structure = Deterministic Discovery`

- **Name**: **Implicit Indexing / Zero-Shot Retrieval**.
- **Frame**: **The Periodic Table**.
  - _Intuition_: Mendeleev didn't have to search the universe for Gallium. He calculated where it _must_ be based on the Periodic Law.
- **The Result**: The agent doesn't need to search (`grep`). It _calculates_ the location of information based on the Axioms. It trusts the **Conceptual Integrity** of the system to be where it says it is.

---

## Summary Checklist

When evaluating a design or documentation:

1.  **Name and Frame**: Have we defined it precisely and intuitively?
2.  **Generative**: Is it a Seed or a Bouquet?
3.  **Canon**: Is it written down?
4.  **Rooted**: Can I find it from the root?
5.  **Distilled**: Is it concise?
