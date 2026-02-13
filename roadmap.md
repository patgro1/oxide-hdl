# Oxide HDL - Development Tracking

**Last Updated:** February 10, 2026

---

## 🚧 Current Work (v0.5)

### In Progress: The "Undeclared" Guard
**Goal:** Stop the user from using things that don't exist. This is the final semantic check needed before the analyzer is "trustworthy".

- [ ] **Undeclared Identifier Diagnostics**
  - [ ] Check `Reference` nodes against `ScopeTree`
  - [ ] Flag unknown signals/variables/constants (Error Severity)
  - [ ] Ignore known built-ins (std_logic, true/false)
- [ ] **Undefined Type Diagnostics**
  - [ ] Flag unknown types in signal declarations (e.g., `signal x : foo;`)
  - [ ] Validate against IEEE library types (already available in `builtins.rs`)

---

## ✅ Completed

### v0.5 Features (Ready)
- [x] **Package System & JIT**
  - [x] `builtins.rs`: Embedded IEEE libraries extraction
  - [x] `workspace.rs`: JIT parsing of dependencies
  - [x] `lookup.rs`: Cross-file symbol resolution
- [x] **Advanced Completion** (Previously planned for v0.8)
  - [x] Context-aware auto-complete
  - [x] Port Map completion (shows ports of the entity)
  - [x] Generic Map completion
  - [x] Dot access completion (`record.field`)

### v0.4 (December 2024)
- [x] Syntax error detection
- [x] Unused signal/constant/variable detection
- [x] Sensitivity list validation
- [x] Entity/Architecture scope extraction
- [x] Constants in type definitions marked as used

---

## 🗺️ Roadmap

### v0.6: Productivity & Smart Snippets
**Goal:** Make writing VHDL faster by automating the tedious parts using the Scope Tree.

- [X] **Smart Auto-Fill Snippets** ✨
  - [X] **Component Instantiation:** Selecting a component in completion triggers a snippet that types out `port map ( clk => $1, rst => $2 ... );`
  - [X] **Procedure/Function Calls:** Auto-fill parameter lists for subprograms.
- [ ] **Rename Symbol**
  - [ ] Rename identifier under cursor
  - [ ] Update definition and all usages across the file
- [ ] **Find All References**
  - [ ] Show all locations where a signal/variable is used
- [ ] **Code Actions (Quick Fixes)**
  - [ ] "Remove unused signal" (using existing unused diagnostic)
  - [ ] "Add to sensitivity list" (using existing sensitivity diagnostic)

### v0.7: Advanced Safety
**Goal:** Catch functional bugs that compile fine but break hardware.

- [ ] **Latch Inference Warning** (If/Case without else)
- [ ] **Multiple Drivers** (Writing to same signal in multiple processes)
- [ ] **Range Validation** (e.g., assigning 8-bit to 4-bit, if statically determinable)

### Future / On Hold
- **Type Mismatches:** Full expression type inference is currently out of scope.
- **Background Worker/Disk Cache:** Current performance (<100ms) does not justify the complexity yet.

---

## 🐛 Known Bugs & Concerns

### Medium Priority
1. **Overloaded Functions:**
   - **Issue:** Function resolution doesn't check signature types, only names.
   - **Status:** Acceptable limitation for now.

2. **Record Type Visibility:**
   - **Issue:** Dot completion relies on text heuristics in some edge cases.
   - **Status:** Working well enough.

---

## 📝 Notes

### Design Decisions Log

1. **Scope Trees as Single Source of Truth**
   - Built once, used for Validation, Hover, Goto, Rename, and Snippet Generation.

2. **JIT Parsing Strategy**
   - Only parse deep structure of files when requested.
   - Keeps startup fast without needing a background worker.

3. **Skipping Type Inference**
   - We focus on "Symbol Resolution" (finding the name) rather than "Type Resolution" (validating the expression).
