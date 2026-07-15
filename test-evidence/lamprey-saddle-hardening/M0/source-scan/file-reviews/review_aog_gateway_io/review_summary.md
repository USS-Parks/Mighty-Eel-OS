# review_aog_gateway_io

All five assigned files were read in full. The review emitted 14 normalized raw candidates: 12 reportable and 2 deferred. High-confidence rows cover Anthropic cross-host redirect credential forwarding, production endpoint/TLS trust, unbounded provider bodies/SSE lines, and provider-controlled usage defeating metering. The two deferred rows preserve deterministic OpenAI/Anthropic streaming false-success semantics pending proof of a privileged downstream consumer.

Exact safe/counterevidence: callers' virtual-key headers are not forwarded; reqwest strips standard Authorization on cross-host redirects (but not Anthropic x-api-key); both surfaces use the same authorize seam; sensitive cloud streams are blocked and non-stream cloud requests are tokenized; no provider retry/fallback loop exists; unsupported multimodal/tool blocks are dropped rather than forwarded around classification.

Deferred gaps: deployment egress ACLs were not inspected; no live adversarial provider harness was run; downstream tool-action reliance on stream termination semantics was not established. Model/route token-caveat enforcement and non-reserving budget findings were already owned by review_aog_gateway_core and were not duplicated.
