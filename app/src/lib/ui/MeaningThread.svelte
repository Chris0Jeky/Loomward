<script lang="ts">
  import type { MeaningThread } from '../contracts.gen';
  import VisibleName from './VisibleName.svelte';
  let { meaning }: { meaning: MeaningThread | undefined } = $props();
  const states: Record<string, string> = { no_model: 'No fitted model', abstained: 'Abstained: neutral', not_eligible: 'Not eligible for student suggestions', suggested: 'Suggested meaning', none: 'No meaning yet', unknown: 'Meaning unknown', pending: 'Meaning pending', labelled: 'Human label', mixed: 'Mixed meaning' };
</script>

<div class="meaning-thread" data-meaning-state={meaning?.state ?? 'unknown'}>
  <p class="state"><i class="weft" aria-hidden="true"></i><span>{meaning ? states[meaning.state] : 'Meaning unknown'}{#if meaning?.label}: <VisibleName name={meaning.label} />{/if}</span></p>
  {#if meaning?.suggestion}
    {#if meaning.suggestion.reasons.length}<p class="muted">{meaning.suggestion.reasons.map(r => r.replaceAll('_', ' ')).join(' · ')}</p>{/if}
    <ol aria-label="Suggested labels and relative scores">
      {#each meaning.suggestion.ranked as item (item.label)}
        <li><VisibleName name={item.label} /> <span class="num">{item.score.toFixed(3)} relative score</span></li>
      {/each}
    </ol>
  {/if}
  <p class="muted note">Suggestions, relative scores, not probabilities, never actions.</p>
</div>

<style>
  p { margin: 0 0 6px; }
  /* the weft mark: crossing threads where there is a label or a suggestion, a dotted stitch where there is none (unknown is not zero) */
  .state { display: flex; align-items: baseline; gap: 8px; font-weight: 600; }
  .weft { flex: none; width: 18px; height: 10px; border: 1px dotted var(--unknown); border-radius: 1px; }
  [data-meaning-state='labelled'] .weft, [data-meaning-state='suggested'] .weft, [data-meaning-state='mixed'] .weft {
    border: 0; background: repeating-linear-gradient(0deg, var(--meaning) 0 2px, transparent 2px 4px);
  }
  .state + p { margin-top: -2px; }
  ol { margin: 6px 0; padding-left: 1.4em; }
  li { padding: 2px 0; overflow-wrap: anywhere; }
  .num { color: var(--muted); }
  .note { font-size: var(--fs-xs); }
</style>
