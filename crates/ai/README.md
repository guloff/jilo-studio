# filmcraft-ai

Cloud AI providers for FilmCraft's AI features (Settings ▸ AI): Google Gemini and OpenAI.

- `Transport`: every request goes through it. `FakeTransport` answers from a script and records
  requests, so tests never use the network; `HttpTransport` (feature `net`) sends them over HTTPS
  with rustls.
- `gemini`, `openai`: JSON answers with a schema (OpenAI strict mode), model lists, key checks, and
  the providers' errors as `AiError` with stable codes (`BAD_API_KEY`, `MODEL_RETIRED`, ...).
- Retries: 3 attempts; 1 s x 2^n after a network failure, 2 s x 2^n after 429, 1.5 s x 2^n after
  5xx. A retired Gemini model whose error names its replacement is replaced once.
- `keys`: `<data dir>/ai.json` (mode 0600, atomic writes) or `GEMINI_API_KEY` / `GOOGLE_API_KEY` /
  `OPENAI_API_KEY`. Not the system keychain. `Secret` never prints.

Nothing here runs unless the user starts an AI feature with a cloud engine; the local engines stay
the default.
