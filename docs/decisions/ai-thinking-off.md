Decision: Every OpenRouter request asks for no thinking (`reasoning.effort: none`); a model that
refuses that is asked once more with it `low`. Up to 4,000 tokens are allowed, and an empty answer
is reported with the model OpenRouter used and its finish reason ("… stopped at the length limit
before answering").

Why: The tasks (picking from a list, turning words into filters) don't need thinking, and a
model's thinking counts against the same token limit as its answer, so a model that thought at
length stopped before answering and was logged only as "said nothing".

Date: 2026-11-02
