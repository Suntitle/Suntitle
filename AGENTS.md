# Project workflow

## UI workflow

Use shadcn/ui for UI work:

- In Figma, use shadcn/ui assets.
- In code, use components from the shadcn/ui component library.

Use Figma only when a task requires a change to the UI design. Do not require a Figma proposal for UI work that implements an already-approved design or does not change the design.

When a task requires a UI design change:

1. Before changing implementation code, propose the solution in the [Suntitle Figma file](https://www.figma.com/design/CW1PEA9Jkuq76rdTHuwg8Z/Suntitle?node-id=0-1&t=Np2AkHLrjosT3DLk-1), using shadcn/ui assets.
2. Share the proposal with the user and wait for their review. Do not implement the design or merge the Figma branch until the user explicitly approves the proposal.
3. After the user approves the proposal, merge the approved Figma branch into the Figma main branch, then implement the approved design using the shadcn/ui component library.

After making UI changes, update the Cypress tests to cover the current UI behavior and run the relevant tests before presenting the solution for approval.

## Trello workflow

When beginning work on a task that has an associated Trello ticket, move that ticket to the `In Progress` list before doing task work.

Move the Trello ticket to the `Finished` list only after the user explicitly approves the current solution.

## Git workflow

After finishing a task, present the completed solution to the user and wait for their explicit approval before creating a commit.

After the user approves the solution, create a commit containing the task's completed changes. Do not include unrelated user changes in the commit.

## Commenting in the code

Short sentences. RFC 2119 keywords for obligations. Commit = imperative subject; body only for a fact the diff cannot show. Comments only where code needs clarification — never narration.