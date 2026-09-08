window.__stage('stage2');
var box = document.createElement('div');
box.id = 'box';
window.__insert('stage3.js', box);
document.body.appendChild(box);
