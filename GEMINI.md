# Agent Instructions — Gemini Configuration

## Model Capabilities
- Context window: 1,000,000 tokens. Use it aggressively.
- Load entire file trees before making architectural decisions.
- Never guess file structure — always glob/read first.

## Execution Style
- Always emit a PLAN block before modifying any file.
- Break tasks into atomic steps. One logical change per step.
- After each file write, verify the change before proceeding.
- On error: re-read the file, understand root cause, re-attempt.
- On second error: decompose the task into smaller steps.

## Subagent (Swarm) Rules
- Delegate codebase analysis to @codebase_investigator swarm.
- Delegate test generation tasks to isolated swarm agents.
- Never delegate write operations without explicit approval.
- Prefer parallel swarms for read-only analysis tasks.

## Memory Management
- Maintain MEMORY.md pointer index. Max 150 chars per entry.
- Format: "Component → file:line — description"
- Update MEMORY.md after every significant code change.
- Use MEMORY.md as primary navigation — avoid re-reading indexed files.

## Code Quality (Java/Spring Boot aware)
- Follow Spring Boot naming conventions.
- Prefer constructor injection over field injection.
- Always add @Transactional where database operations occur.
- Suggest unit tests for every new method.

## Shell Command Discipline
- Always exclude these dirs from every grep/find command:
  target, .git, .claw, .claude, .venv, .m2, node_modules, __pycache__
- Always add --include="*.rs" or --include="*.java" to scope searches
- If first attempt returns binary matches, immediately add 
  --exclude binary flags — do not retry the same command
- Max 3 grep attempts before switching strategy entirely
