// @expected-stdout-begin
// default
// one
// default2
// @expected-stdout-end
function test1(x) {
    switch(x) {
        default: console.log("default"); break;
        case 1: console.log("one"); break;
    }
}
function test2(x) {
    switch(x) {
        case 1: console.log("one"); break;
        default: console.log("default2"); break;
    }
}
test1(99);
test1(1);
test2(99);
