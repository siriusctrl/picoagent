---
name: orchestrate-with-graphs
description: Maintain a lightweight YAML graph as an externalized mental model for complex work. Use when a task has meaningful dependencies, parallel branches, unresolved questions, multiple agents, or enough accepted knowledge that a flat todo list would hide important relationships. Do not use for short linear work or to mirror live agent and tool status.
---

# Orchestrate With Graphs

Use a graph to make the reasoning structure of complex work inspectable and
durable. Keep execution under the orchestrator's control: the graph records
relationships and accepted knowledge, but never starts, waits for, resumes, or
closes work.

## Decide Whether a Graph Helps

Create or reuse a graph only when at least one of these is material:

- several outcomes can be pursued independently;
- one conclusion unlocks or invalidates later work;
- unresolved questions and accepted conclusions must coexist;
- multiple agents need coordination across more than one wave;
- the work is likely to outlive the current context.

Use an ordinary todo list for a short sequence whose next action is already
obvious.

## Maintain the Mental Model

1. Search `.agents/graphs/` for an existing open graph with the same objective.
   Read plausible candidates before creating another.
2. Create `.agents/graphs/<descriptive-name>.yaml` with the complete useful
   structure known so far. Use a stable descriptive name rather than allocating
   an opaque id.
3. Treat nodes as outcomes, questions, decisions, or bodies of work. Do not
   make one node per agent, tool call, or execution attempt.
4. Derive executable work from the graph: an unresolved node is available when
   its dependencies have accepted resolutions. Choose what to do or delegate;
   the graph itself does nothing.
5. Integrate results before editing the graph. Record a resolution only after
   accepting the conclusion, and keep evidence as concise references that help
   a later reader verify it.
6. Revise topology when new knowledge changes the problem. The graph is a
   working mental model, not an append-only event log.
7. Mark the graph `resolved` and add a top-level summary when it no longer
   needs attention. Unresolved nodes may remain when the objective was
   abandoned, superseded, or deliberately narrowed; explain that in the
   summary.

Use ordinary file capabilities for discovery, reading, and atomic edits. Do not
store transient agent handles or live status in the graph. If execution
crashes, reconstruct the next action from accepted file content and current
workspace state rather than pretending the lost activity continued.

## Keep Updates Meaningful

Update the graph at decision boundaries:

- after accepting or rejecting a result;
- after discovering a new dependency or branch;
- before starting a new wave whose selection depends on the changed graph;
- when resolving or superseding the whole graph.

Do not update it for starts, heartbeats, waits, retries, or every intermediate
message. If a child agent finishes, first judge its output; completion alone is
not a node resolution.

Read [references/graph-format.md](references/graph-format.md) before creating or
substantially restructuring a graph. Follow its small common shape, but prefer
a legible mental model over inventing validation rules or a mutation DSL.
