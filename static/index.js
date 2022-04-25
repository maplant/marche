$(document).ready(function() {
    var tagHLcolor = "";
    var threadHLcolor = "";
    if ($(".tag").length > 0)
    {
        tagHLcolor = darkenRGBString(window.getComputedStyle($(".tag")[0])["background-color"], 0.90);
    }
    if ($("li.menu-item").length > 0)
    {
        threadHLcolor = darkenRGBString(window.getComputedStyle($("li.menu-item")[0])["background"], 0.96);
    }
    $(".tag").hover(function() {
        $(this).css({ 'background-color' : tagHLcolor});
        $(this).parents(".thread-row").css({ 'background-color' : ''});
    }, function() {
        $(this).css({ 'background-color' : ''});
        $(this).parents(".thread-row").css({ 'background-color' : threadHLcolor});
    });
    $(".thread-row").hover(function() {
        $(this).parents(".thread-row").css({ 'background-color' : ''});
        $(this).css({ 'background-color' : threadHLcolor});
    }, function() {
        $(this).css({ 'background-color' : ''});
    });
});

function darkenRGBString(rgb, factor)
{
    rgb = rgb.replace(/\).*/g, '').replace(/[^\d,.]/g, '').split(',');
    return `rgb(${rgb[0]*factor}, ${rgb[1]*factor}, ${rgb[2]*factor})`
}