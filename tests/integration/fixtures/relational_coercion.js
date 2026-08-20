// Relational operator type coercion edge cases
// @expected-stdout-begin
// true
// true
// true
// true
// false
// false
// false
// false
// true
// @expected-stdout-end
console.log(null < 1);             // true (ToNumber(null) = 0, 0 < 1)
console.log(null >= 0);            // true (0 >= 0)
console.log(null <= 0);            // true (0 <= 0)
console.log(null > -1);            // true (0 > -1)
console.log(undefined < 0);        // false (NaN comparison)
console.log(undefined > 0);        // false (NaN comparison)
console.log(undefined <= 0);       // false (NaN comparison)
console.log(undefined >= 0);       // false (NaN comparison)
console.log(true > false);         // true (1 > 0)
