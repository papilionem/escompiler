// Corpus entry: arith-add-overflow
// Expectation taken from the pinned Node oracle (.ai/facts/node-pin.json),
// NOT hand-typed. Every other fixture's expectation was typed by a person;
// this is the first set generated from the external oracle.
//
// EXPECTED TO FAIL until R1-03 deletes the I32 arithmetic specialisation.
// That failure is the deliverable: when it flips, the harness goes red until
// the line is removed from tests/integration/xfail.txt.
// @expected-stdout: 4000000000
console.log(2e9 + 2e9);
