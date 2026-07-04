"""MAI Ollama adapter.

Full implementation: chat/completion streaming, embeddings,
model management, GPU layer assignment, health checking.

"""

from adapters.ollama.adapter import OllamaAdapter
from adapters.ollama.config import OllamaConfig

__all__ = ["OllamaAdapter", "OllamaConfig"]
