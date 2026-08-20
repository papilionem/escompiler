// @expected-stdout-begin
// f
// g
// named
//
// @expected-stdout-end
const f = function() {};
console.log(f.name);
let g = () => {};
console.log(g.name);
var h = function named() {};
console.log(h.name);
console.log((function(){}).name);
