// @expected-stdout: 2,,12,13,14
// Corrected 2026-08-12 from `2,11,12,13,14`, which was hand-typed and wrong:
// vNull receives null (null is not undefined, so the default does NOT apply —
// as this fixture's own comment below says), and Array#join renders null as the
// empty string. Verified against the pinned Node oracle. The compiler was right
// and the expectation was wrong, so this sat in the XFAIL registry accusing
// working code. Found by the Node differential, which is the only check that
// can see a wrong expectation.
var v2, vNull, vHole, vUndefined, vOob;
for ([v2 = 10, vNull = 11, vHole = 12, vUndefined = 13, vOob = 14] of [[2, null, undefined, undefined]]) {
  // v2 gets 2 (value present, not undefined)
  // vNull gets null (null is not undefined, so no default)
  // vHole gets 12 (undefined triggers default)
  // vUndefined gets 13 (undefined triggers default)
  // vOob gets 14 (out of bounds = undefined, triggers default)
}
console.log([v2, vNull, vHole, vUndefined, vOob].join(","));
