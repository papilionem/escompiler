// @expected-stdout: 6
function make() {
    return (x) => x + 1;
}
console.log(make()(5));
