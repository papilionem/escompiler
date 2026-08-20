// @expected-stdout-begin
// 1
// 99
// @expected-stdout-end
// eval inside closure modifies captured variable
function outer() {
    var x = 1;
    var modify = function() {
        eval("x = 99");
    };
    console.log(x);
    modify();
    console.log(x);
}
outer();
