import unittest
from loomward.learning import Student, validate_features

def event(i,name,label,source='human',revision=1,retracted=False):
    return {'event_id':str(i)+source+str(revision),'item_id':str(i),'revision':revision,'source':source,'label':label,
            'features':{'name':name,'extension':'.txt','context':'','size_bytes':100},'retracted':retracted}

def examples():
    return [event('a','invoice payment account','Finance'),event('b','invoice receipt payment','Finance'),
            event('c','photo portrait camera','Media'),event('d','photo image camera','Media')]

class LearningTests(unittest.TestCase):
    def test_empty_model_abstains(self):
        s=Student(['Finance','Media']); s.fit([])
        self.assertTrue(s.predict({'name':'invoice'})['abstain'])
    def test_genuinely_learns_examples(self):
        s=Student(['Finance','Media']); s.fit(examples())
        r=s.predict({'name':'invoice payment','extension':'.txt','size_bytes':100})
        self.assertEqual(r['suggestions'][0]['label'],'Finance')
        self.assertFalse(r['calibrated']); self.assertFalse(r['autonomy_allowed'])
    def test_opposite_labels_change_prediction(self):
        s=Student(['Finance','Media']); rows=examples()
        for r in rows: r['label']='Media' if r['label']=='Finance' else 'Finance'
        s.fit(rows)
        self.assertEqual(s.predict({'name':'invoice payment'})['suggestions'][0]['label'],'Media')
    def test_unknown_vocabulary_abstains(self):
        s=Student(['Finance','Media']); s.fit(examples())
        self.assertTrue(s.predict({'name':'xyzzq blorp'})['abstain'])
    def test_teacher_only_cannot_escape_abstention(self):
        rows=examples()
        for r in rows: r['source']='teacher'
        s=Student(['Finance','Media']); s.fit(rows)
        self.assertTrue(s.predict({'name':'invoice payment'})['abstain'])
    def test_human_label_dominates_teacher(self):
        rows=examples()+[event('a','invoice payment account','Media','teacher',99)]
        s=Student(['Finance','Media']); s.fit(rows)
        self.assertEqual(s.training_count,4)
        self.assertEqual(s.human_support['Finance'],2)
    def test_retraction_does_not_resurrect_teacher_label(self):
        rows=[event('a','invoice','Finance','teacher'),event('a','invoice','Finance','human',2,True)]
        s=Student(['Finance','Media']); s.fit(rows); self.assertEqual(s.training_count,0)
    def test_unknown_label_rejected(self):
        s=Student(['Finance','Media'])
        with self.assertRaises(ValueError): s.fit([event('a','x','NotAllowed')])
    def test_raw_content_field_rejected(self):
        with self.assertRaises(ValueError): validate_features({'name':'x','content':'secret'})
    def test_conflicting_same_revision_rejected(self):
        s=Student(['Finance','Media'])
        with self.assertRaises(ValueError): s.fit([event('a','x','Finance'),event('a','x','Media')])
    def test_scores_sum_to_one(self):
        s=Student(['Finance','Media']); s.fit(examples())
        r=s.predict({'name':'invoice'})
        self.assertAlmostEqual(sum(x['score'] for x in r['suggestions']),1)
    def test_model_roundtrip(self):
        s=Student(['Finance','Media']); s.fit(examples()); restored=Student.from_dict(s.to_dict())
        self.assertEqual(s.predict({'name':'invoice'}),restored.predict({'name':'invoice'}))
