"""Small, inspectable supervised learner. Scores are not calibrated probabilities."""
from __future__ import annotations
import copy
import hashlib
import json
import math
import re
from collections import Counter
from typing import Any
from .inventory import integer

WEIGHTS = {'human': 1.0, 'teacher': 0.2}
FEATURE_KEYS = frozenset({'name', 'extension', 'context', 'size_bytes'})
DEFAULT_LABELS = ['Documents', 'Finance', 'Media', 'Projects', 'Models', 'Archive']


def validate_labels(labels: Any) -> list[str]:
    if not isinstance(labels, list) or not 1 <= len(labels) <= 32:
        raise ValueError('Taxonomy must have 1..32 labels')
    if any(not isinstance(x, str) or not re.fullmatch(r'[A-Za-z][A-Za-z0-9 _-]{0,63}', x) for x in labels):
        raise ValueError('Labels must be short names, not paths or commands')
    if len(set(labels)) != len(labels):
        raise ValueError('Duplicate label')
    return list(labels)


def validate_features(features: Any) -> dict[str, Any]:
    if not isinstance(features, dict) or set(features) - FEATURE_KEYS:
        raise ValueError('Only bounded name, extension, context and size metadata are permitted')
    result: dict[str, Any] = {}
    for key, limit in [('name', 256), ('extension', 32), ('context', 512)]:
        value = features.get(key, '')
        if not isinstance(value, str) or len(value) > limit or '\x00' in value:
            raise ValueError(f'Invalid {key}')
        result[key] = value
    result['size_bytes'] = integer(features.get('size_bytes', 0), 'size_bytes')
    return result


def tokens(features: dict[str, Any]) -> set[str]:
    f = validate_features(features)
    result: set[str] = set()
    for key, prefix in [('name', 'word'), ('context', 'context')]:
        for word in re.findall(r'[^\W_]+', f[key].casefold(), re.UNICODE)[:64]:
            if len(word) > 1 and not word.isdecimal():
                result.add(f'{prefix}:{word}')
    if f['extension']:
        result.add('extension:' + f['extension'].casefold().lstrip('.'))
    if f['size_bytes']:
        result.add('size_bucket:' + str(int(math.log2(f['size_bytes'])) // 4))
    return result


def _canonical(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True, allow_nan=False)


class Student:
    """Weighted multinomial Naive Bayes reference model with explicit abstention."""
    def __init__(self, labels: list[str] | None = None):
        self.labels = validate_labels(labels if labels is not None else DEFAULT_LABELS)
        self.fit([])

    def fit(self, events: list[dict[str, Any]]) -> 'Student':
        if not isinstance(events, list) or len(events) > 10_000:
            raise ValueError('At most 10000 feedback events per prototype training run')
        latest: dict[tuple[str, str], dict[str, Any]] = {}
        normalised = []
        event_ids: dict[str, str] = {}
        for raw in events:
            if not isinstance(raw, dict):
                raise ValueError('Feedback event must be an object')
            source = raw.get('source')
            if source not in WEIGHTS:
                raise ValueError('Unknown supervision source')
            item = raw.get('item_id')
            event_id = raw.get('event_id')
            if not isinstance(item, str) or not 1 <= len(item) <= 128 or not isinstance(event_id, str) or not 1 <= len(event_id) <= 128:
                raise ValueError('Invalid item or event ID')
            revision = integer(raw.get('revision'), 'revision', 1, 10**9)
            label = raw.get('label')
            if label not in self.labels:
                raise ValueError('Label is outside the taxonomy')
            retracted = raw.get('retracted', False)
            if type(retracted) is not bool:
                raise ValueError('retracted must be boolean')
            r = {'event_id': event_id, 'item_id': item, 'revision': revision, 'source': source,
                 'label': label, 'features': validate_features(raw.get('features')), 'retracted': retracted}
            serial = _canonical(r)
            if event_id in event_ids:
                if event_ids[event_id] != serial:
                    raise ValueError('Conflicting feedback event ID')
                continue
            event_ids[event_id] = serial
            normalised.append(r)
            key = (item, source)
            previous = latest.get(key)
            if previous and previous['revision'] == revision:
                left = {k: v for k, v in previous.items() if k != 'event_id'}
                right = {k: v for k, v in r.items() if k != 'event_id'}
                if left != right:
                    raise ValueError('Conflicting feedback at the same revision')
            if previous is None or revision > previous['revision']:
                latest[key] = r
        selected = []
        for item in sorted({key[0] for key in latest}):
            # An explicit human retraction suppresses the item, including older teacher labels.
            chosen = latest.get((item, 'human')) or latest.get((item, 'teacher'))
            if chosen and not chosen['retracted']:
                selected.append(chosen)
        counts = {label: Counter() for label in self.labels}
        mass = {label: 0.0 for label in self.labels}
        human_support = {label: 0 for label in self.labels}
        vocabulary: set[str] = set()
        for r in selected:
            weight = WEIGHTS[r['source']]
            terms = tokens(r['features'])
            vocabulary.update(terms)
            if len(vocabulary) > 100_000:
                raise ValueError('Prototype vocabulary budget exceeded')
            for term in terms:
                counts[r['label']][term] += weight
            mass[r['label']] += weight
            if r['source'] == 'human':
                human_support[r['label']] += 1
        # Commit the new fitted state only after the entire training set validates.
        self._events = normalised
        self.counts, self.mass, self.human_support = counts, mass, human_support
        self.vocabulary = vocabulary
        self.training_count = len(selected)
        self.model_id = hashlib.sha256(_canonical({'labels': self.labels, 'selected': selected}).encode()).hexdigest()[:16]
        return self

    def predict(self, features: dict[str, Any]) -> dict[str, Any]:
        all_terms = tokens(features)
        known = all_terms & self.vocabulary
        coverage = len(known) / max(1, len(all_terms))
        v = max(1, len(self.vocabulary))
        total_mass = sum(self.mass.values())
        logits = {}
        for label in self.labels:
            prior = (self.mass[label] + 1.0) / (total_mass + len(self.labels))
            denominator = sum(self.counts[label].values()) + v
            logits[label] = math.log(prior) + sum(math.log((self.counts[label][term] + 1.0) / denominator) for term in known)
        highest = max(logits.values())
        exp = {label: math.exp(logit - highest) for label, logit in logits.items()}
        normaliser = sum(exp.values())
        suggestions = sorted(({'label': label, 'score': value / normaliser} for label, value in exp.items()),
                             key=lambda x: (-x['score'], x['label']))
        first = suggestions[0]
        margin = first['score'] - (suggestions[1]['score'] if len(suggestions) > 1 else 0)
        entropy = -sum(x['score'] * math.log(max(x['score'], 1e-15)) for x in suggestions)
        entropy /= math.log(max(2, len(suggestions)))
        reasons = []
        if self.training_count == 0:
            reasons.append('no_training_examples')
        if sum(self.human_support.values()) < 4 or self.human_support[first['label']] < 2:
            reasons.append('insufficient_human_support')
        if not known or coverage < 0.35:
            reasons.append('unfamiliar_features')
        if first['score'] < 0.8 or margin < 0.2:
            reasons.append('ambiguous_model_scores')
        return {'model_id': self.model_id, 'suggestions': suggestions, 'abstain': bool(reasons),
                'reasons': reasons, 'feature_coverage': round(coverage, 6), 'known_features': sorted(known)[:12],
                'review_priority': round(min(1.0, 0.5 * entropy + 0.35 * (1 - coverage) + 0.15 * bool(reasons)), 6),
                'calibrated': False, 'autonomy_allowed': False,
                'score_note': 'Uncalibrated relative model scores, not a probability that a move is safe.'}

    def to_dict(self) -> dict[str, Any]:
        return {'schema_version': 1, 'algorithm': 'weighted_multinomial_nb_v1', 'labels': list(self.labels),
                'events': copy.deepcopy(self._events), 'model_id': self.model_id, 'calibrated': False}

    @classmethod
    def from_dict(cls, value: Any) -> 'Student':
        if not isinstance(value, dict) or type(value.get('schema_version')) is not int or value['schema_version'] != 1 or value.get('algorithm') != 'weighted_multinomial_nb_v1':
            raise ValueError('Unsupported model format')
        return cls(value.get('labels')).fit(value.get('events'))
