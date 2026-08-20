// @expected-stdout-begin
// 10
// 20
// true
// @expected-stdout-end
function Point(x, y) {
    this.x = x;
    this.y = y;
}
var p = Reflect.construct(Point, [10, 20]);
console.log(p.x);
console.log(p.y);
console.log(p instanceof Point);
