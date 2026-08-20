// @expected-stdout: Rex barks
class Animal {
  constructor(name) {
    this.name = name;
  }
  speak() {
    return this.name + " speaks";
  }
}
class Dog extends Animal {
  speak() {
    return this.name + " barks";
  }
}
let d = new Dog("Rex");
console.log(d.speak());
