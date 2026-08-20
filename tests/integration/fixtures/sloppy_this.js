// @expected-stdout-begin
// object
// undefined
// @expected-stdout-end
function f() { return typeof this; }
console.log(f());
var g = function() {
    "use strict";
    return typeof this;
};
console.log(g());
