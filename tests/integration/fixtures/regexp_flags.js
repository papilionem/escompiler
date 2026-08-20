// @expected-stdout-begin
// true
// true
// gi
// @expected-stdout-end
let re = /abc/gi;
console.log(re.global);
console.log(re.ignoreCase);
console.log(re.flags);
