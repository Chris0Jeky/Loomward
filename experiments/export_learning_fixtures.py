#!/usr/bin/env python3
"""Export parity fixtures for the Rust student port (crates/loomward-learn) from the Python reference.

The oracle is python/loomward/learning.py, which this script never edits. Run

    py -3 experiments/export_learning_fixtures.py          # regenerate fixtures/learning-v3/
    py -3 experiments/export_learning_fixtures.py --check  # regenerate in memory and compare

--check compares every field exactly except floats, which must agree within 1e-12 relative.
Floats may differ in the last bits between runs because CPython iterates sets in a hash-seeded
order when it sums log terms, so an exact byte comparison would be flaky.
"""
from __future__ import annotations

import argparse
import json
import math
import random
import sys
import unicodedata
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'python'))

from loomward import learning  # noqa: E402
from loomward.learning import DEFAULT_LABELS, Student, tokens, validate_features  # noqa: E402

OUT = ROOT / 'fixtures' / 'learning-v3'
SCHEMA = 'loomward-learn-fixtures/1'
SEED = 20261009
TOKEN_FILE = 'tokens.json'
STUDENT_FILE = 'student.json'
REQUIRED_REASONS = {'no_training_examples', 'insufficient_human_support', 'unfamiliar_features', 'ambiguous_model_scores'}


def feat(name: str = '', extension: str = '', context: str = '', size_bytes: int = 0) -> dict[str, Any]:
    return {'name': name, 'extension': extension, 'context': context, 'size_bytes': size_bytes}


# ---------------------------------------------------------------- token fixtures

CURATED_TOKEN_FEATURES: list[dict[str, Any]] = [
    feat('Invoice 2024.pdf', '.pdf', 'Finance Accounts', 2048),
    feat('Straße plan', '', '', 0),
    feat('STRASSE plan', '', '', 0),
    feat('GROẞ', '', '', 0),                       # capital sharp s folds to "ss"
    feat('İstanbul trip', '', '', 0),              # dotted capital I folds to i + combining dot, which splits
    feat('ıstanbul trip', '', '', 0),              # dotless i has no folding and is a word character
    feat('ΟΔΟΣ ΣΊΣΥΦΟΣ', '', '', 0),               # Greek capital sigma and final sigma fold to sigma
    feat('οδος ς', '', '', 0),
    feat('café', '', '', 0),                 # combining acute splits the word
    feat('Café au lait', '', '', 0),
    feat('हिन्दी', '', '', 0),                        # Devanagari vowel signs (Mn/Mc) split every run
    feat('नमस्ते दुनिया', '', '', 0),
    feat('東京 report', '', '', 0),                   # CJK letters are word characters
    feat('東', '', '', 0),                            # a single CJK letter is shorter than two characters
    feat('一二三 numbers', '', '', 0),                # Han numerals are letters (Lo), not decimal digits
    feat('二〇二四 archive', '', '', 0),               # ideographic zero is a letter number (Nl)
    feat('٢٠٢٤ arabic digits', '', '', 0),           # decimal digits only: dropped as decimal words
    feat('abc٢٠٢٤', '', '', 0),
    feat('x² superscript', '', '', 0),                # superscript two is a number (No), a word character
    feat('２０２４ fullwidth', '', '', 0),            # fullwidth digits are decimal
    feat('ＡＢＣ fullwidth letters', '', '', 0),
    feat('ﬁle ligature', '', '', 0),                  # the fi ligature folds to two letters
    feat('my_file__name', '', '', 0),
    feat('report-2024.v2.final', '', '', 0),
    feat('a b c', '', '', 0),                         # single letters are dropped
    feat('(final) [draft] {v2}', '', '', 0),
    feat('📄report', '', '', 0),                       # an emoji separates words
    feat('zero​width joiner', '', '', 0),        # zero-width space is a format character (Cf)
    feat('nbsp separated', '', '', 0),
    feat('tab\tnew\nline', '', '', 0),
    feat('ǅungla', '', '', 0),                        # titlecase digraph folds to dž
    feat('ꭰꭰ Cherokee', '', '', 0),                   # Cherokee capitals fold to lowercase
    feat('KK Kelvin', '', '', 0),                # Kelvin sign folds to k
    feat('Ångström Ångstrom', '', '', 0),  # Angstrom sign folds to a with ring
    feat('\U0001D400\U0001D401\U0001D402 math', '', '', 0),  # mathematical bold capitals stay as they are
    feat('\U00010400\U00010401 deseret', '', '', 0),  # astral capitals fold through a surrogate pair
    feat('Project Loomward src', '', 'Projects Loomward src', 0),
    feat('report', '', 'report report', 0),           # the same word in name and context yields both prefixes
    feat('', '', '', 0),                              # empty features: no tokens at all
    feat('2024', '', '', 0),                          # a decimal-only word is dropped
    feat('0 1 2 3', '', '', 0),
    feat('abc2024', '', '', 0),
    feat('file', '.PDF', '', 0),
    feat('file', '..jpg', '', 0),
    feat('file', '.', '', 0),                         # a lone dot leaves an empty extension token
    feat('file', 'JPEG', '', 0),
    feat('file', '', '', 0),
    feat('file', '.İ', '', 0),                   # extension casefolds to i + combining dot
    feat('file', '', '', 1),
    feat('file', '', '', 2),
    feat('file', '', '', 15),
    feat('file', '', '', 16),
    feat('file', '', '', 255),
    feat('file', '', '', 256),
    feat('file', '', '', 1 << 20),
    feat('file', '', '', (1 << 53) - 1),
    feat('file', '', '', 1 << 52),
    feat(' '.join(chr(97 + i // 26) + chr(97 + i % 26) for i in range(70)), '', '', 0),  # 70 words: only the first 64 count
    feat('x' * 256, '', 'y ' * 255 + 'zz', 0),
]


def random_token_features(rng: random.Random) -> dict[str, Any]:
    stems = ['invoice', 'contract', 'photo', 'holiday', 'backup', 'model', 'report', 'bank', 'tax',
             'src', 'loom', 'archive', 'video', 'notes', 'budget', 'draft', 'final', 'Straße', 'İzmir',
             'ΟΔΟΣ', 'café', 'हिन्दी', '東京', 'データ', 'Ärger', 'résumé', 'NAÏVE', 'ǅungla', '２０２４',
             '٢٠٢٤', 'x²', 'ﬁle', '📄', 'Ångström', 'ß', 'ΣΊΣΥΦΟΣ', 'a', '1', '42', 'v2']
    seps = [' ', '_', '-', '.', '  ', ' + ', '/', '\t']
    words = [rng.choice(stems) for _ in range(rng.randint(0, 6))]
    name = ''
    for word in words:
        name += word + rng.choice(seps)
    name = name.rstrip(' ').strip()[:256]
    extension = rng.choice(['', '.pdf', 'PDF', '..md', '.Jpeg', '.ÄÖ', '.ß', '.', '.2024', '.東'])
    context = ' '.join(rng.choice(stems) for _ in range(rng.randint(0, 4)))[:512]
    size = rng.choice([0, 1, 3, 7, 8, 9, 31, 4096, 10_000_000, rng.randint(0, (1 << 53) - 1)])
    return feat(name, extension, context, size)


def token_cases() -> list[dict[str, Any]]:
    rng = random.Random(SEED)
    features = list(CURATED_TOKEN_FEATURES)
    while len(features) < 230:
        features.append(random_token_features(rng))
    cases = []
    for index, item in enumerate(features):
        cases.append({'id': f'tok-{index:03d}', 'features': validate_features(item),
                      'tokens': sorted(tokens(item))})
    return cases


# ---------------------------------------------------------------- student fixtures

def ev(event_id: str, item: str, label: str, source: str = 'human', revision: int = 1,
       retracted: bool = False, **features: Any) -> dict[str, Any]:
    return {'event_id': event_id, 'item_id': item, 'label': label, 'source': source, 'revision': revision,
            'retracted': retracted, 'features': feat(**features) if features else feat()}


DOC_NAMES = ['invoice march', 'contract signed', 'letter to bank', 'meeting notes', 'annual report', 'tax receipt']
FIN_NAMES = ['bank statement', 'salary slip', 'budget plan', 'tax return', 'credit card receipt', 'investment report']
MEDIA_NAMES = ['holiday photo', 'beach video', 'album cover', 'family photo', 'concert video', 'camera raw']
PROJECT_NAMES = ['loomward src', 'build script', 'repo readme', 'module design', 'test fixture', 'cargo manifest']


def bulk_events(rng: random.Random, count: int) -> list[dict[str, Any]]:
    pools = {'Documents': DOC_NAMES, 'Finance': FIN_NAMES, 'Media': MEDIA_NAMES, 'Projects': PROJECT_NAMES}
    out = []
    for index in range(count):
        label = rng.choice(sorted(pools))
        name = rng.choice(pools[label])
        item = f'item-{index:03d}'
        source = 'teacher' if rng.random() < 0.35 else 'human'
        revision = rng.randint(1, 3)
        retracted = rng.random() < 0.08
        out.append({'event_id': f'bulk-{index:03d}-r{revision}', 'item_id': item, 'label': label, 'source': source,
                    'revision': revision, 'retracted': retracted,
                    'features': feat(name, rng.choice(['.pdf', '.jpg', '.xlsx', '.txt']),
                                     rng.choice(['Documents Scans', 'Media Phone', 'Projects Loomward src', 'Finance 2024']),
                                     rng.choice([0, 2048, 1 << 20, 3_000_000]))})
    return out


def standard_queries() -> list[dict[str, Any]]:
    return [
        feat('bank receipt 2024.pdf', '.pdf', 'Finance Records', 4096),
        feat('holiday video.mp4', '.mp4', 'Media Trip', 1 << 25),
        feat('zzqx ymtrk', '.zz', 'unknown folder', 99),
        feat('', '', '', 0),
        feat('invoice', '.pdf', '', 0),
        feat('loomward src builder', '.rs', 'Projects Loomward', 128),
        feat('Straße', '', '', 0),
        feat('photo', '', 'Media Photo Holiday', 1 << 20),
    ]


def scenario_inputs() -> list[dict[str, Any]]:
    rng = random.Random(SEED + 1)
    scenarios: list[dict[str, Any]] = []

    def add(case_id: str, events: Any, labels: Any = None, queries: list[dict[str, Any]] | None = None,
            serialise: bool = False) -> None:
        scenarios.append({'id': case_id, 'labels': labels, 'events': events,
                          'queries': queries if queries is not None else standard_queries(),
                          'serialise': serialise})

    add('empty_default', [], queries=standard_queries())
    add('human_only_three', [ev(f'h{i}', f'h{i}', 'Finance', name=n) for i, n in enumerate(FIN_NAMES[:3])])
    add('human_supported_clear', [
        ev(f'd{i}', f'doc-{i}', 'Documents', name=n, extension='.pdf') for i, n in enumerate(DOC_NAMES)
    ] + [
        ev(f'f{i}', f'fin-{i}', 'Finance', name=n, extension='.pdf') for i, n in enumerate(FIN_NAMES)
    ] + [
        ev(f'm{i}', f'med-{i}', 'Media', name=n, extension='.mp4') for i, n in enumerate(MEDIA_NAMES)
    ], serialise=True)
    add('teacher_weight_only', [
        ev(f't{i}', f'tch-{i}', 'Projects', source='teacher', name=n) for i, n in enumerate(PROJECT_NAMES)
    ] + [ev('h0', 'human-0', 'Documents', name='invoice march')])
    add('human_overrides_teacher', [
        ev('t1', 'same', 'Media', source='teacher', name='holiday photo'),
        ev('h1', 'same', 'Finance', name='bank statement'),
        ev('h2', 'other', 'Finance', name='tax return'),
        ev('h3', 'third', 'Finance', name='budget plan'),
        ev('h4', 'fourth', 'Documents', name='invoice march'),
        ev('h5', 'fifth', 'Documents', name='contract signed'),
    ])
    add('human_retraction_suppresses_teacher', [
        ev('t1', 'x', 'Media', source='teacher', revision=1, name='holiday photo'),
        ev('h1', 'x', 'Finance', revision=2, retracted=True, name='bank statement'),
        ev('t2', 'y', 'Projects', source='teacher', revision=1, name='build script'),
        ev('h2', 'z', 'Documents', name='invoice march'),
    ])
    add('teacher_retraction', [
        ev('t1', 'x', 'Media', source='teacher', revision=1, retracted=True, name='holiday photo'),
        ev('t2', 'x', 'Media', source='teacher', revision=2, retracted=True, name='holiday photo'),
        ev('t3', 'y', 'Projects', source='teacher', revision=1, name='build script'),
        ev('h1', 'z', 'Documents', name='invoice march'),
    ])
    add('revision_supersedes', [
        ev('a', 'item', 'Documents', revision=1, name='invoice march'),
        ev('b', 'item', 'Finance', revision=3, name='bank statement'),
        ev('c', 'item', 'Media', revision=2, name='holiday photo'),
        ev('d', 'item2', 'Media', revision=1, name='beach video'),
        ev('e', 'item2', 'Media', revision=1, name='beach video'),
    ])
    add('same_revision_identical_keeps_first_event_id', [
        ev('first', 'dup', 'Finance', revision=1, name='tax return'),
        ev('second', 'dup', 'Finance', revision=1, name='tax return'),
        ev('h2', 'a', 'Documents', name='invoice march'),
        ev('h3', 'b', 'Documents', name='contract signed'),
        ev('h4', 'c', 'Media', name='holiday photo'),
        ev('h5', 'd', 'Projects', name='build script'),
    ])
    add('identical_event_id_deduplicates', [
        ev('same', 'a', 'Finance', name='tax return'),
        ev('same', 'a', 'Finance', name='tax return'),
        ev('h2', 'b', 'Media', name='holiday photo'),
    ])
    add('conflicting_event_id', [
        ev('same', 'a', 'Finance', name='tax return'),
        ev('same', 'a', 'Media', name='holiday photo'),
    ])
    add('same_revision_conflict', [
        ev('e1', 'a', 'Finance', revision=2, name='tax return'),
        ev('e2', 'a', 'Media', revision=2, name='holiday photo'),
    ])
    add('same_revision_conflict_features', [
        ev('e1', 'a', 'Finance', revision=2, name='tax return'),
        ev('e2', 'a', 'Finance', revision=2, name='tax letter'),
    ])
    add('lower_revision_conflict_ignored', [
        ev('e1', 'a', 'Finance', revision=5, name='tax return'),
        ev('e2', 'a', 'Media', revision=4, name='holiday photo'),
    ])
    add('custom_taxonomy', [
        ev(f'w{i}', f'work-{i}', 'Work', name=n) for i, n in enumerate(['quarterly report', 'client meeting', 'project plan'])
    ] + [
        ev(f'p{i}', f'home-{i}', 'Home photos', name=n) for i, n in enumerate(['family photo', 'birthday party', 'holiday photo'])
    ] + [ev('w9', 'work-9', 'Work', name='sprint review')],
        labels=['Work', 'Home photos'],
        queries=[feat('client report'), feat('birthday photo'), feat('zzqx')])
    add('bulk_mixed_seed', bulk_events(rng, 90), queries=standard_queries() + [
        feat('invoice march', '.pdf', 'Documents Scans', 2048),
        feat('build script', '.rs', 'Projects Loomward src', 1 << 20),
    ], serialise=True)
    add('ambiguous_shared_terms', [
        ev('a1', 'a1', 'Documents', name='shared alpha'),
        ev('a2', 'a2', 'Documents', name='shared alpha'),
        ev('a3', 'a3', 'Finance', name='shared alpha'),
        ev('a4', 'a4', 'Finance', name='shared alpha'),
        ev('a5', 'a5', 'Media', name='shared beta'),
        ev('a6', 'a6', 'Media', name='shared beta'),
    ], queries=[feat('shared alpha'), feat('shared'), feat('beta')])
    add('unicode_model_terms', [
        ev('u1', 'u1', 'Documents', name='Straße Plan', context='Dokumente'),
        ev('u2', 'u2', 'Documents', name='İstanbul trip', context='Reisen'),
        ev('u3', 'u3', 'Finance', name='東京 receipt', context='財務'),
        ev('u4', 'u4', 'Finance', name='ΟΔΟΣ budget'),
        ev('u5', 'u5', 'Media', name='café photo'),
        ev('u6', 'u6', 'Media', name='nafé video'),
    ], queries=[feat('STRASSE plan'), feat('istanbul'), feat('東京'), feat('οδος'), feat('café photo')])
    add('long_name_word_cap', [
        ev('long1', 'l1', 'Projects', name=' '.join('wx%02d' % i for i in range(80))),
        ev('long2', 'l2', 'Projects', name='build script'),
        ev('h1', 'l3', 'Documents', name='invoice march'),
        ev('h2', 'l4', 'Finance', name='bank statement'),
        ev('h3', 'l5', 'Finance', name='tax return'),
    ], queries=[feat(' '.join('wx%02d' % i for i in range(64, 80))), feat(' '.join('wx%02d' % i for i in range(0, 8)))])
    add('size_buckets_and_extensions', [
        {'event_id': 's1', 'item_id': 's1', 'label': 'Archive', 'source': 'human', 'revision': 1, 'retracted': False,
         'features': feat('snapshot zip', '.ZIP', 'Backups', 1 << 33)},
        {'event_id': 's2', 'item_id': 's2', 'label': 'Archive', 'source': 'human', 'revision': 1, 'retracted': False,
         'features': feat('legacy archive', '.zip', 'Backups', 16)},
        {'event_id': 's3', 'item_id': 's3', 'label': 'Models', 'source': 'human', 'revision': 1, 'retracted': False,
         'features': feat('weights checkpoint', '.gguf', 'Models', (1 << 53) - 1)},
        {'event_id': 's4', 'item_id': 's4', 'label': 'Models', 'source': 'human', 'revision': 1, 'retracted': False,
         'features': feat('tokenizer model', '.gguf', 'Models', 256)},
        {'event_id': 's5', 'item_id': 's5', 'label': 'Documents', 'source': 'human', 'revision': 1, 'retracted': False,
         'features': feat('invoice march', '.pdf', 'Docs', 0)},
    ], queries=[feat('zip archive', '.zip', 'Backups', 1 << 33), feat('model weights', '.gguf', '', 256),
                feat('notes', '', '', 1)])
    add('abstain_few_labels_only_unfamiliar', [
        ev('a', 'a', 'Documents', name='invoice'),
        ev('b', 'b', 'Documents', name='contract'),
        ev('c', 'c', 'Finance', name='bank'),
        ev('d', 'd', 'Finance', name='budget'),
    ], queries=[feat('unrelatedterm'), feat('invoice', '', '', 0)])
    add('insufficient_support_for_first_label', [
        ev('a', 'a', 'Documents', name='invoice march'),
        ev('b', 'b', 'Documents', name='contract signed'),
        ev('c', 'c', 'Finance', name='bank statement'),
        ev('d', 'd', 'Finance', name='tax return'),
        ev('e', 'e', 'Media', name='holiday photo'),
        ev('f', 'f', 'Projects', name='build script'),
        ev('g', 'g', 'Archive', name='snapshot zip'),
        ev('h', 'h', 'Models', name='weights gguf'),
    ], queries=[feat('invoice march'), feat('build script'), feat('weights gguf')])
    add('empty_tokens_query_only', [ev('x', 'x', 'Documents', name='invoice march'), ev('y', 'y', 'Documents', name='contract signed'),
                                    ev('z', 'z', 'Finance', name='bank statement'), ev('w', 'w', 'Finance', name='tax return')],
        queries=[feat('', '', '', 0), feat('a', '.', '', 0)])
    return scenarios


def error_scenarios() -> list[dict[str, Any]]:
    good = ev('ok', 'ok', 'Documents', name='invoice march')
    cases: list[dict[str, Any]] = []

    def bad(case_id: str, event: Any, **extra: Any) -> None:
        cases.append({'id': case_id, 'labels': None, 'events': [event], 'queries': [feat('invoice march')],
                      'serialise': False, **extra})

    bad('err_not_object', 'x')
    bad('err_unknown_source', {**good, 'source': 'robot'})
    bad('err_missing_source', {k: v for k, v in good.items() if k != 'source'})
    bad('err_empty_item', {**good, 'item_id': ''})
    bad('err_long_event_id', {**good, 'event_id': 'e' * 129})
    bad('err_item_not_string', {**good, 'item_id': 7})
    bad('err_revision_zero', {**good, 'revision': 0})
    bad('err_revision_float', {**good, 'revision': 1.0})
    bad('err_revision_bool', {**good, 'revision': True})
    bad('err_revision_too_big', {**good, 'revision': 10**9 + 1})
    bad('err_label_outside', {**good, 'label': 'Pictures'})
    bad('err_label_not_string', {**good, 'label': 3})
    bad('err_retracted_int', {**good, 'retracted': 1})
    bad('err_features_missing', {k: v for k, v in good.items() if k != 'features'})
    bad('err_features_extra_key', {**good, 'features': {**good['features'], 'content': 'x'}})
    bad('err_name_too_long', {**good, 'features': feat('n' * 257)})
    bad('err_context_nul', {**good, 'features': feat('name', '', 'a\u0000b')})
    bad('err_extension_too_long', {**good, 'features': feat('name', '.' + 'e' * 32)})
    bad('err_size_negative', {**good, 'features': feat('name', '', '', -1)})
    bad('err_size_too_big', {**good, 'features': feat('name', '', '', 1 << 53)})
    bad('err_size_float', {**good, 'features': {**good['features'], 'size_bytes': 2.0}})
    bad('err_order_source_before_label', {**good, 'source': 'robot', 'label': 'Pictures'})
    cases.append({'id': 'err_fit_not_list', 'labels': None, 'events': None, 'queries': [feat('invoice march')],
                  'serialise': False})
    return cases


def label_error_scenarios() -> list[dict[str, Any]]:
    bad_labels: list[tuple[str, Any]] = [
        ('labels_empty', []),
        ('labels_too_many', [f'Label{i}' for i in range(33)]),
        ('labels_path', ['Documents/Scans']),
        ('labels_leading_digit', ['1Documents']),
        ('labels_non_ascii', ['Ünïcode']),
        ('labels_too_long', ['A' * 65]),
        ('labels_duplicate', ['Work', 'Work']),
        ('labels_not_list', 'Work'),
        ('labels_non_string', ['Work', 4]),
        ('labels_trailing_newline', ['Work\n']),
    ]
    out = []
    for case_id, labels in bad_labels:
        out.append({'id': case_id, 'labels': labels, 'events': [], 'queries': [feat('x')], 'serialise': False,
                    'labels_error_expected': True})
    out.append({'id': 'labels_max_length_ok', 'labels': ['A' * 64, 'B b-_ 2'], 'events': [],
                'queries': [feat('x')], 'serialise': False})
    out.append({'id': 'labels_all_32_ok', 'labels': [f'L{i}' for i in range(32)], 'events': [],
                'queries': [feat('x')], 'serialise': False})
    return out


def run_fit(case: dict[str, Any]) -> tuple[Student | None, dict[str, Any]]:
    """Mirror the reference call order: construct with labels, then fit the events."""
    record: dict[str, Any] = {'labels_error': None, 'fit_error': None}
    try:
        student = Student(case['labels'])
    except ValueError as exc:
        record['labels_error'] = str(exc)
        return None, record
    try:
        student.fit(case['events'])
    except ValueError as exc:
        record['fit_error'] = str(exc)
    return student, record


def describe(student: Student) -> dict[str, Any]:
    stored = student.to_dict()
    return {
        'model_id': student.model_id,
        'training_count': student.training_count,
        'human_support': {label: student.human_support[label] for label in student.labels},
        'mass': {label: student.mass[label] for label in student.labels},
        'vocabulary_size': len(student.vocabulary),
        'stored_events': stored['events'],
        'labels': student.labels,
    }


def predict_record(student: Student, query: dict[str, Any]) -> dict[str, Any]:
    try:
        result = student.predict(query)
    except ValueError as exc:
        return {'features': query, 'error': str(exc)}
    return {'features': query, 'result': result}


def student_case(case: dict[str, Any]) -> dict[str, Any]:
    student, record = run_fit(case)
    out: dict[str, Any] = {'id': case['id'], 'labels': case['labels'], 'input_events': case['events'],
                           'labels_error': record['labels_error'], 'fit_error': record['fit_error']}
    if case.get('labels_error_expected') and record['labels_error'] is None:
        raise SystemExit(f"{case['id']}: expected a label error")
    if student is None:
        return out
    out['after_fit'] = describe(student)
    out['predictions'] = [predict_record(student, q) for q in case['queries']]
    if case.get('serialise'):
        out['serialised'] = student.to_dict()
        restored = Student.from_dict(student.to_dict())
        out['round_trip_model_id'] = restored.model_id
    return out


def from_dict_cases() -> list[dict[str, Any]]:
    base = Student(['Work', 'Home photos'])
    base.fit([ev('a', 'a', 'Work', name='quarterly report'), ev('b', 'b', 'Home photos', name='family photo')])
    good = base.to_dict()
    cases = []
    for case_id, value in [
        ('from_dict_ok', good),
        ('from_dict_schema_version_2', {**good, 'schema_version': 2}),
        ('from_dict_schema_version_bool', {**good, 'schema_version': True}),
        ('from_dict_wrong_algorithm', {**good, 'algorithm': 'other_v1'}),
        ('from_dict_not_object', ['x']),
        ('from_dict_missing_events', {k: v for k, v in good.items() if k != 'events'}),
        ('from_dict_missing_labels', {k: v for k, v in good.items() if k != 'labels'}),
    ]:
        try:
            restored = Student.from_dict(value)
            record = {'model_id': restored.model_id, 'training_count': restored.training_count,
                      'serialised': restored.to_dict()}
        except ValueError as exc:
            record = {'error': str(exc)}
        cases.append({'id': case_id, 'input': value, 'expect': record})
    return cases


def build() -> dict[str, Any]:
    tokens_doc = {'schema': SCHEMA, 'source': 'python/loomward/learning.py', 'python_unicode': unicodedata.unidata_version,
                  'cases': token_cases()}
    student_cases = [student_case(c) for c in scenario_inputs()]
    error_cases = []
    for case in error_scenarios():
        error_cases.append(student_case(case))
    label_cases = [student_case(c) for c in label_error_scenarios()]
    student_doc = {'schema': SCHEMA, 'source': 'python/loomward/learning.py',
                   'python_unicode': unicodedata.unidata_version, 'weights': learning.WEIGHTS,
                   'default_labels': DEFAULT_LABELS, 'cases': student_cases + error_cases + label_cases,
                   'from_dict': from_dict_cases()}
    return {TOKEN_FILE: tokens_doc, STUDENT_FILE: student_doc}


def coverage(docs: dict[str, Any]) -> None:
    """Refuse to write fixtures that do not exercise what the brief requires."""
    tokens_cases = docs[TOKEN_FILE]['cases']
    if len(tokens_cases) < 200:
        raise SystemExit(f'only {len(tokens_cases)} token cases; the brief requires at least 200')
    seen_reasons: set[str] = set()
    teacher_weight_seen = False
    error_messages: set[str] = set()
    for case in docs[STUDENT_FILE]['cases']:
        if case.get('fit_error'):
            error_messages.add(case['fit_error'])
        if case.get('labels_error'):
            error_messages.add(case['labels_error'])
        for prediction in case.get('predictions', []):
            if 'result' in prediction:
                seen_reasons.update(prediction['result']['reasons'])
        if case.get('after_fit') and any(isinstance(e, dict) and e.get('source') == 'teacher' for e in case['input_events'] or []):
            teacher_weight_seen = True
    missing = REQUIRED_REASONS - seen_reasons
    if missing:
        raise SystemExit(f'abstention reasons never exercised: {sorted(missing)}')
    if not teacher_weight_seen:
        raise SystemExit('no teacher-weighted case')
    needed = {'Conflicting feedback event ID', 'Conflicting feedback at the same revision',
              'Label is outside the taxonomy', 'Unknown supervision source'}
    if not needed <= error_messages:
        raise SystemExit(f'error messages missing: {sorted(needed - error_messages)}')


def canonical_text(doc: dict[str, Any]) -> str:
    return json.dumps(doc, sort_keys=True, indent=1, ensure_ascii=True, allow_nan=False) + '\n'


def close(a: Any, b: Any) -> bool:
    if isinstance(a, float) or isinstance(b, float):
        return isinstance(a, (int, float)) and isinstance(b, (int, float)) and math.isclose(a, b, rel_tol=1e-12, abs_tol=0.0)
    if type(a) is not type(b):
        return False
    if isinstance(a, dict):
        return a.keys() == b.keys() and all(close(a[k], b[k]) for k in a)
    if isinstance(a, list):
        return len(a) == len(b) and all(close(x, y) for x, y in zip(a, b))
    return a == b


def first_difference(a: Any, b: Any, path: str = '$') -> str | None:
    if close(a, b):
        return None
    if isinstance(a, dict) and isinstance(b, dict):
        for key in sorted(set(a) | set(b)):
            if key not in a or key not in b:
                return f'{path}.{key}: present on one side only'
            found = first_difference(a[key], b[key], f'{path}.{key}')
            if found:
                return found
    if isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            return f'{path}: length {len(a)} != {len(b)}'
        for index, (x, y) in enumerate(zip(a, b)):
            found = first_difference(x, y, f'{path}[{index}]')
            if found:
                return found
    return f'{path}: {a!r} != {b!r}'


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--check', action='store_true', help='compare regenerated fixtures with the committed files')
    args = parser.parse_args()
    docs = build()
    coverage(docs)
    if args.check:
        failed = False
        for name, doc in docs.items():
            path = OUT / name
            if not path.exists():
                print(f'missing {path.relative_to(ROOT).as_posix()}; run without --check')
                failed = True
                continue
            committed = json.loads(path.read_text(encoding='utf-8'))
            difference = first_difference(committed, json.loads(canonical_text(doc)))
            if difference:
                print(f'{path.relative_to(ROOT).as_posix()} differs at {difference}')
                failed = True
            else:
                print(f'ok {path.relative_to(ROOT).as_posix()}')
        return 1 if failed else 0
    OUT.mkdir(parents=True, exist_ok=True)
    for name, doc in docs.items():
        path = OUT / name
        path.write_text(canonical_text(doc), encoding='utf-8', newline='\n')
        print(f'wrote {path.relative_to(ROOT).as_posix()} ({len(doc["cases"])} cases)')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
