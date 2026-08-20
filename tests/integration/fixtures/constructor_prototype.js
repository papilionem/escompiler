// @expected-stdout-begin
// PASS
// @expected-stdout-end
function Animal(name) {
    this.name = name;
}
Animal.prototype.speak = function() {
    return this.name + " speaks";
};
var a = new Animal("Dog");
if (a.name !== "Dog") throw "FAIL: name is " + a.name;
if (a.speak() !== "Dog speaks") throw "FAIL: speak is " + a.speak();
console.log("PASS");
