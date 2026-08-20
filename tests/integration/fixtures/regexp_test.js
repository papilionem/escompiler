// @expected-stdout-begin
// true
// false
// @expected-stdout-end
let re = /abc/;
console.log(re.test("xabcx"));
console.log(re.test("xyz"));
