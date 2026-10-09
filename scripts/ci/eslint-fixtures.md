# ESLint fixture proofs

Intent of the Bun test next to `scripts/test_eslint.py`. Callers still run the Python file.

| Original | New | Why |
| --- | --- | --- |
| `unittest` subTest records a failure and keeps going through the remaining cases. | The first mismatch throws, and the rest of that proof stops. | A passing proof still runs every case. The failure is the proof's result. |
| `subprocess` inherits stdin when no `input` is passed. | stdin is `ignore` unless a fixture string is passed. | The child must not read the test runner's stdin. `--stdin` fixtures are still the bytes given to ESLint. |
| `print-config` is used only after the process exits 0. | `printConfig` also requires exit 0 before it reads JSON. | A failed `print-config` must not be treated as a config object. |
| `Path.glob("*.vue.d.ts")` on Python 3.12 includes leading-dot names. | `readdirSync` lists every directory entry, including dotfiles, then keeps names ending in `.vue.d.ts`. | The count includes a hidden declaration on purpose. The editor tree has none, so the count stays 8. |
