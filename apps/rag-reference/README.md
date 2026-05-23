# RAG Reference

A local retrieval-augmented generation demo: documents are embedded
and stored in memory, a query retrieves the most relevant chunks by
cosine similarity, and a local chat model answers using those chunks
as context. The full pipeline -- embed, retrieve, generate -- runs
on the local node with no external vector database and no cloud call.

The pipeline: local text docs -> embed -> cosine top-k -> chat with
retrieved context.

## What It Demonstrates

- Batched embedding via `client.embed(model, [chunks])`
- In-memory `VectorStore` with cosine similarity ranking
- Two-stage SDK use: embed pipeline followed by chat with system context
- Per-stage error handling that distinguishes ingest failures from
  answer failures

## Run

```powershell
mkdir apps/rag-reference/sample_docs
"The MAI server runs locally on port 8420." | Out-File -Encoding utf8 apps/rag-reference/sample_docs/about.md

python apps/rag-reference/main.py "What port does MAI use?"
```

Expected output:

```
[ingest] 1 document, 1 chunk embedded (embed-v1)
[retrieve] top-1 chunk: "The MAI server runs locally on port 8420." (score=0.97)
[answer] The MAI server runs on port 8420.
```

## Configure

Edit [`config.toml`](config.toml). The defaults assume:

- An `embed-v1` model that the server reports with
  `capabilities.embedding = true`.
- A `qwen3-14b:Q4_K_M` chat model.

Change `[ingest] docs_dir` to point at your own corpus.

## Tests

```powershell
pytest apps/rag-reference/tests/
```

`test_smoke.py` -- VectorStore cosine math, chunking helper, end-to-end
run with mocked server.

`test_integration.py` -- top-k ranking quality with three competing
chunks.

## Deliberate Design Choices

These constraints keep the demo focused and legible. They are not gaps
to close before running the demo.

- **In-memory only.** No persistence between runs. This is intentional:
  the demo proves the retrieval pipeline, not the storage layer. A
  production deployment can swap in `mai-vault::VectorStore` once the
  vector store endpoint is exposed over HTTP.
- **Naive chunker.** Character-window splitting, not token-aware.
  Production RAG typically uses sentence or paragraph segmentation;
  this demo keeps the chunking logic visible and trivial so the
  retrieval stage is easy to inspect.
- **No reranker.** Top-k is direct cosine similarity. A second-stage
  reranker improves precision in production but adds complexity that
  obscures the two-stage SDK pattern this demo exists to show.
- **No streaming.** The final answer is generated in one shot. Adding
  `chat_stream()` here is a one-line change; it is omitted so the
  embed-then-chat structure reads cleanly.
