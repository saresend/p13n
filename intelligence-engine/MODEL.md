# Building the decision model

The server calls one function:

```
predict(schema, recent_actions) -> (confidence, proposed_actions)
```

Right now it's `IdentityModel`, which proposes back what it was given with confidence 0. These
notes cover how to replace it, from simplest to most ambitious, so we can write it by hand.

## What problem this actually is

Your example: writing agent task descriptions that are part bespoke, part boilerplate (how to
structure PRs, manage tickets). The system should notice the boilerplate and offer it. That splits
into two different problems, and it's worth keeping them apart:

1. **Structure: which action comes next.** "After `create` you usually `write_text`, then
   `save`." This is *next-action prediction*, the same shape as next-word prediction in language
   models or "next item" in recommenders. The vocabulary is small (a handful of action names), so
   simple counting goes a long way.
2. **Content: what the payload should be.** "Your task notes almost always contain this PR
   paragraph." This is about repeated *content* across actions. For a notes app that means
   finding text that recurs across documents. This is where your paved-path example actually
   lives, and it's closer to deduplication and template mining than to neural nets.

A good first model does both with counting and hashing, no training loop at all. A neural model is
a later step, worth it only once the simple one has shown where it falls short.

## Level 1: n-gram model over action names (structure)

Treat each user's history as a sentence whose words are action names:

```
create write_text write_text save select write_text save create write_text …
```

Count how often each name follows each context of the last *k* names:

```
counts[("write_text", "save")]["create"] += 1
```

Then P(next = a | context) = counts[context][a] / total(context). Predict the most likely next
action; its probability is a natural confidence. To propose several steps, roll forward greedily:
append the prediction to the context, predict again, and multiply the probabilities. That product
falls fast, which is correct behavior: long guesses *should* have low confidence.

Things to learn by building it:

- **Smoothing.** An unseen context has zero counts. Add-one (Laplace) smoothing, or **backoff**:
  if the 3-name context is rare, fall back to 2 names, then 1. Backoff is about 15 lines and is the
  heart of classic language modeling.
- **Training is just counting.** You can update counts on every new action (*online learning*),
  so there's no separate training job.
- **Repeated actions.** `write_text` fires on every keystroke, which floods the counts. Collapsing
  runs (`write_text×40` → `write_text`) before counting is *featurization*: deciding what the
  model sees. In practice it matters more than the model choice.

## Level 2: pattern mining for paved paths (structure + content)

N-grams answer "what's next"; *paved paths* are longer routines the user repeats. Mine them
directly:

1. Slide windows of length 2–8 over the collapsed history and count each sequence (its
   *support*).
2. Keep sequences seen at least *s* times. Those are candidate routines.
3. At predict time, if the end of the current history matches the **start** of a frequent
   routine, propose the **rest** of it. Confidence ≈ support(full routine) / support(matched
   prefix).

For content, do the same over text instead of action names:

1. Split each saved note into paragraphs (or sentences) and normalize them (lowercase, collapse
   whitespace).
2. Hash each paragraph and count how many *different notes* contain it. Paragraphs that appear in
   several notes are boilerplate.
3. For near-duplicates (the PR paragraph with one word changed), use **shingling**: hash every
   run of 5 words. Two paragraphs are similar if they share most shingles (Jaccard similarity).
   **MinHash** lets you estimate that cheaply once there are many notes.
4. Suggest: when a note is created, or its first lines resemble notes that contained boilerplate,
   propose a `write_text` whose payload is the current text plus the boilerplate.

That last step is your example almost exactly, and it's maybe 150 lines of Rust with no
dependencies.

## Level 3: a small learned sequence model

When counting stops being enough (long-range patterns, fuzzy similarity, many users), the next step
is a small neural sequence model:

- **Tokenize actions.** Each action becomes tokens: an action-name token plus payload features (a
  text embedding, or "same note as before?" flags). The JSON Schema gives you the token
  vocabulary, which is why `/init` exists.
- **Model.** A small GRU or a 2–4 layer transformer with hidden size ~64–128. That's small enough
  to run on CPU inside the server.
- **Objective.** Next-token cross-entropy, the same as a language model, trained on users'
  histories.
- **Train offline, run embedded.** Train in PyTorch, export the weights, and run inference in
  Rust. Either write the forward pass by hand (a GRU is a few matrix multiplies, a good exercise)
  or use `candle`, or ONNX through `tract` or `ort`. That's the "compiled embedded model" in the
  spec.
- **Respect the schema while generating.** Mask out tokens that would produce an invalid action
  (*constrained decoding*). The engine already drops invalid proposals, but a model that never
  produces them wastes no confidence on them.

The hard parts here are data, not architecture. One person produces relatively few actions, so
expect to pre-train on synthetic or pooled data and then adapt per user, or to stay with Levels 1–2
for a long time.

## Confidence and evaluation

`confidence` only helps if it means something. Let 0.8 mean "right about 80% of the time":

- **Offline evaluation.** Replay a user's log in time order. At each point, predict, then check
  against what actually happened next. Report top-1 and top-3 accuracy. Never evaluate on data the
  model has already counted.
- **Calibration.** Bucket predictions by confidence and compare each bucket's average confidence
  with its accuracy (a *reliability diagram*). Count-based models are often overconfident on rare
  contexts; a minimum support threshold helps.
- **Threshold in the client.** Show suggestions only above some confidence, tuned so they're
  welcome rather than noisy.

## Feedback loop

- Suggestions come back with `source: "system"`. Keep them **out of training data**, or the model
  learns from its own guesses.
- Add `accept_suggestion` and `dismiss_suggestion` actions to the vocabulary. They're the most
  valuable signal you'll get: explicit labels for what was helpful.

## One design change a real model will need

`predict` currently sees only the actions since the last run (at most 100). Levels 1–3 all need
*history*: counts, mined routines, or a hidden state accumulated over time. Two options:

1. Give the trait a learning hook, e.g. `observe(&mut self, user_id, action)` alongside
   `predict`, and keep per-user state in the model. This is better for online counting models.
2. Pass the full history (or a long window) to `predict` and keep it stateless. Simpler, but it
   recomputes everything each tick.

I'd go with (1) for Level 1. Changing the trait is a small, contained edit in `src/model.rs` and
`src/engine.rs`.

## Suggested order

1. Change the trait (option 1 above) and write an `NGramModel` with backoff. Unit-test it on
   hand-written sequences.
2. Write the offline evaluator (replay a log, report accuracy and calibration). Every later model
   gets measured with it.
3. Add paragraph hashing for boilerplate in `write_text` and `save` payloads (Level 2). This is the
   feature from your example.
4. Add `accept_suggestion` and `dismiss_suggestion` to the editor and the vocabulary.
5. Only then consider Level 3.
